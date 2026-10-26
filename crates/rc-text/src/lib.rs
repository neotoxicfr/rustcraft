//! Game text (`docs/specs/text.md` § 3–4): components received as NBT, inherited styles,
//! translations, legacy § codes, measuring and splitting into lines. Independent of rendering: the
//! advances come from a function supplied by the caller (fonts).

use hashbrown::HashMap;
use std::sync::Arc;

use rc_proto::nbt::Nbt;

pub mod bidi;

/// Named colours, in the order of the § codes 0–f.
pub const COLORS: [(&str, u32); 16] = [
    ("black", 0x000000),
    ("dark_blue", 0x0000AA),
    ("dark_green", 0x00AA00),
    ("dark_aqua", 0x00AAAA),
    ("dark_red", 0xAA0000),
    ("dark_purple", 0xAA00AA),
    ("gold", 0xFFAA00),
    ("gray", 0xAAAAAA),
    ("dark_gray", 0x555555),
    ("blue", 0x5555FF),
    ("green", 0x55FF55),
    ("aqua", 0x55FFFF),
    ("red", 0xFF5555),
    ("light_purple", 0xFF55FF),
    ("yellow", 0xFFFF55),
    ("white", 0xFFFFFF),
];

/// Three-state style: `None` = inherited from the parent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    pub color: Option<u32>,
    pub shadow_color: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underlined: Option<bool>,
    pub strikethrough: Option<bool>,
    pub obfuscated: Option<bool>,
    pub font: Option<String>,
    /// Click action (`click_event`); shared (cheap copy per character).
    pub click: Option<Arc<ClickEvent>>,
    /// Hover tooltip (`hover_event`).
    pub hover: Option<Arc<HoverEvent>>,
    /// Text inserted by Shift + click (`insertion`).
    pub insertion: Option<Arc<str>>,
}

/// Click action of a text: kind (`change_page`, `run_command`, `open_url`…) and value;
/// `nbt` carries the `show_dialog` dialog (identifier or content) or the `custom` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ClickEvent {
    pub action: String,
    pub value: String,
    pub nbt: Option<Nbt>,
}

/// Hover tooltip of a text (`docs/specs/text.md` § 3.6).
#[derive(Debug, Clone, PartialEq)]
pub enum HoverEvent {
    /// Text (component).
    Text(Component),
    /// Item: identifier, count, data components (NBT, `minecraft:…` keys).
    Item { id: String, count: i32, components: Option<Nbt> },
    /// Entity: type, UUID, optional name.
    Entity { id: String, uuid: u128, name: Option<Component> },
}

impl Style {
    /// Fields defined here, otherwise those of the parent.
    pub fn inherit(&self, parent: &Style) -> Style {
        Style {
            color: self.color.or(parent.color),
            shadow_color: self.shadow_color.or(parent.shadow_color),
            bold: self.bold.or(parent.bold),
            italic: self.italic.or(parent.italic),
            underlined: self.underlined.or(parent.underlined),
            strikethrough: self.strikethrough.or(parent.strikethrough),
            obfuscated: self.obfuscated.or(parent.obfuscated),
            font: self.font.clone().or_else(|| parent.font.clone()),
            click: self.click.clone().or_else(|| parent.click.clone()),
            hover: self.hover.clone().or_else(|| parent.hover.clone()),
            insertion: self.insertion.clone().or_else(|| parent.insertion.clone()),
        }
    }
}

/// Resolved style of a character.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharStyle {
    pub color: Option<u32>,
    pub shadow_color: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
    /// Font (empty = `minecraft:default`).
    pub font: String,
    /// Click action, hover and insertion inherited from the component.
    pub click: Option<Arc<ClickEvent>>,
    pub hover: Option<Arc<HoverEvent>>,
    pub insertion: Option<Arc<str>>,
}

impl From<&Style> for CharStyle {
    fn from(s: &Style) -> Self {
        CharStyle {
            color: s.color,
            shadow_color: s.shadow_color,
            bold: s.bold.unwrap_or(false),
            italic: s.italic.unwrap_or(false),
            underlined: s.underlined.unwrap_or(false),
            strikethrough: s.strikethrough.unwrap_or(false),
            obfuscated: s.obfuscated.unwrap_or(false),
            font: s.font.clone().unwrap_or_default(),
            click: s.click.clone(),
            hover: s.hover.clone(),
            insertion: s.insertion.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    Translate {
        key: String,
        fallback: Option<String>,
        with: Vec<Arg>,
    },
    Keybind(String),
    Selector(String),
    /// Object drawn in the text (atlas sprite, player head) and fallback text for
    /// plain-text uses.
    Object {
        info: ObjectInfo,
        fallback: Option<Box<Component>>,
    },
    /// Score, nbt: nothing on the client side (resolved by the server).
    Nothing,
}

/// Content of an `object` component.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectInfo {
    /// Sprite `sprite` of atlas `atlas` (full identifiers; default block atlas).
    Atlas { atlas: String, sprite: String },
    /// Player head, with or without the hat layer: profile rewritten like the game on
    /// reading (bare name, or compound: name, identifier, properties, imposed skin).
    Player { profile: Box<Nbt>, hat: bool },
}

/// Object carried by the font of a flattened character.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObjectFont<'a> {
    Atlas { atlas: &'a str, sprite: &'a str },
    /// Player head: default skin drawn from `uuid` (identifier, or `Err(nickname)` whose
    /// identifier the caller draws offline); skin to request `(key, profile)`; imposed
    /// resource (`body`, empty otherwise).
    Player { hat: bool, uuid: Result<u128, &'a str>, request: Option<(u128, &'a str)>, body: &'a str },
}

/// Identity of a head without an identifier (profile hash), like that of placed heads.
pub fn head_hash(s: &str) -> u128 {
    let mut h: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    for b in s.bytes() {
        h ^= u128::from(b);
        h = h.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013B);
    }
    h | 1 << 127
}

/// Field separator of a head font (absent from names, identifiers and properties).
const FIELD: char = '\u{1F}';

/// Useful fields of a head profile: name, identifier, `textures` property, imposed skin.
struct Profile<'a> {
    name: Option<&'a str>,
    id: Option<u128>,
    textures: Option<&'a str>,
    body: Option<String>,
}

impl<'a> Profile<'a> {
    fn of(p: &'a Nbt) -> Profile<'a> {
        if let Nbt::String(s) = p {
            return Profile { name: Some(s), id: None, textures: None, body: None };
        }
        let textures = match p.get("properties") {
            Some(Nbt::List(l)) => l
                .iter()
                .find(|e| e.get("name").and_then(Nbt::as_str) == Some("textures"))
                .and_then(|e| e.get("value")?.as_str()),
            _ => None,
        };
        Profile {
            name: p.get("name").and_then(Nbt::as_str),
            id: p.get("id").and_then(nbt_uuid),
            textures,
            body: p.get("texture").and_then(Nbt::as_str).map(full_id),
        }
    }
}

const BLOCKS_ATLAS: &str = "minecraft:blocks";

fn full_id(s: &str) -> String {
    if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") }
}

impl ObjectInfo {
    /// Default fallback text (`[sprite]`, `[sprite@atlas]`, `[name head]`), like the game.
    pub fn default_fallback(&self) -> String {
        let short = |id: &str| id.strip_prefix("minecraft:").unwrap_or(id).to_owned();
        match self {
            ObjectInfo::Atlas { atlas, sprite } if atlas == BLOCKS_ATLAS => format!("[{}]", short(sprite)),
            ObjectInfo::Atlas { atlas, sprite } => format!("[{}@{}]", short(sprite), short(atlas)),
            ObjectInfo::Player { profile, .. } => match Profile::of(profile).name {
                Some(n) => format!("[{n} head]"),
                None => "[unknown player head]".to_owned(),
            },
        }
    }

    /// Reserved font that carries the object in a flattened character (U+FFFC), for rendering.
    ///
    /// Head: default skin of the identifier (otherwise that of an offline player of that name,
    /// otherwise the null identifier); skin requested only for a `textures` property, or for
    /// a profile reduced to a name or an identifier (resolved by the profile service), like
    /// a placed head.
    pub fn font(&self) -> String {
        match self {
            ObjectInfo::Atlas { atlas, sprite } => format!("{OBJECT_FONT}{atlas}|{sprite}"),
            ObjectInfo::Player { profile, hat } => {
                let Profile { name, id, textures, body } = Profile::of(profile);
                let uuid = match (id, name) {
                    (Some(u), _) => format!("{u:032x}"),
                    (None, Some(n)) => format!("name:{n}"),
                    (None, None) => format!("{:032x}", 0),
                };
                let (key, spec) = match (textures, name, id) {
                    (Some(t), _, _) => (id.unwrap_or_else(|| head_hash(t)), t.to_owned()),
                    (None, Some(n), None) => (head_hash(&format!("name:{}", n.to_lowercase())), format!("name:{n}")),
                    (None, None, Some(u)) => (u, format!("id:{u:032x}")),
                    _ => (0, String::new()),
                };
                let body = body.unwrap_or_default();
                let hat = u8::from(*hat);
                format!("{OBJECT_FONT}player|{hat}{FIELD}{uuid}{FIELD}{key:032x}{FIELD}{body}{FIELD}{spec}")
            }
        }
    }
}

/// Prefix of the fonts reserved for objects drawn in the text (`rustcraft:object/` then
/// `atlas|sprite`, or `player|…`); such a character advances by 8 and is drawn at 8 × 8.
pub const OBJECT_FONT: &str = "rustcraft:object/";
/// Character carrying an object in the flattened text.
pub const OBJECT_CHAR: char = '\u{FFFC}';

/// Object of an object font.
pub fn object_font(font: &str) -> Option<ObjectFont<'_>> {
    let rest = font.strip_prefix(OBJECT_FONT)?;
    if let Some(p) = rest.strip_prefix("player|") {
        let mut f = p.splitn(5, FIELD);
        let hat = f.next()? == "1";
        let who = f.next()?;
        let uuid = match who.strip_prefix("name:") {
            Some(n) => Err(n),
            None => Ok(u128::from_str_radix(who, 16).unwrap_or(0)),
        };
        let key = u128::from_str_radix(f.next()?, 16).ok()?;
        let body = f.next()?;
        let spec = f.next()?;
        let request = (!spec.is_empty()).then_some((key, spec));
        return Some(ObjectFont::Player { hat, uuid, request, body });
    }
    let (atlas, sprite) = rest.split_once('|')?;
    Some(ObjectFont::Atlas { atlas, sprite })
}

/// Key bound to each action (`key.jump` → `key.keyboard.space`), for `keybind`
/// components; set by the game at each change of keys.
static KEYBINDS: std::sync::RwLock<Option<HashMap<String, String>>> = std::sync::RwLock::new(None);

pub fn set_keybinds(binds: impl IntoIterator<Item = (String, String)>) {
    *KEYBINDS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(binds.into_iter().collect());
}

/// Label of a key like the game: single character in uppercase, otherwise its translation;
/// mouse button without translation: `key.mouse` with its number.
pub fn key_label(lang: &Lang, key: &str) -> String {
    if let Some(rest) = key.strip_prefix("key.keyboard.")
        && rest.chars().count() == 1
    {
        return rest.to_uppercase();
    }
    if let Some(t) = lang.map.get(key) {
        return t.clone();
    }
    match key.strip_prefix("key.mouse.") {
        Some(n) => match lang.map.get("key.mouse") {
            Some(t) => t.replacen("%1$s", n, 1).replacen("%s", n, 1),
            None => key.to_owned(),
        },
        None => key.to_owned(),
    }
}

/// Text of a `keybind` component: name of the bound key, otherwise translation of the action name.
pub fn keybind_text(lang: &Lang, action: &str) -> String {
    let bound = KEYBINDS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .and_then(|m| m.get(action).cloned());
    match bound {
        Some(key) => key_label(lang, &key),
        None => lang.map.get(action).cloned().unwrap_or_else(|| action.to_owned()),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Component(Component),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub content: Content,
    pub style: Style,
    pub extra: Vec<Component>,
}

impl Component {
    pub fn text(s: impl Into<String>) -> Component {
        Component { content: Content::Text(s.into()), style: Style::default(), extra: Vec::new() }
    }

    pub fn translate(key: impl Into<String>, with: Vec<Arg>) -> Component {
        Component {
            content: Content::Translate { key: key.into(), fallback: None, with },
            style: Style::default(),
            extra: Vec::new(),
        }
    }
}

/// Colour of a component: name or `#hexa` (0..0xFFFFFF).
pub fn parse_color(s: &str) -> Option<u32> {
    if let Some(h) = s.strip_prefix('#') {
        return u32::from_str_radix(h, 16).ok().filter(|&v| v <= 0xFF_FFFF);
    }
    COLORS.iter().find(|c| c.0 == s).map(|c| c.1)
}

fn nbt_bool(v: &Nbt) -> Option<bool> {
    match v {
        Nbt::Byte(b) => Some(*b != 0),
        Nbt::Int(i) => Some(*i != 0),
        _ => None,
    }
}

fn nbt_text(v: &Nbt) -> Option<String> {
    Some(match v {
        Nbt::String(s) => s.clone(),
        Nbt::Byte(b) => b.to_string(),
        Nbt::Short(b) => b.to_string(),
        Nbt::Int(b) => b.to_string(),
        Nbt::Long(b) => b.to_string(),
        Nbt::Float(b) => java_number(f64::from(*b), format!("{b:e}"), b.to_string()),
        Nbt::Double(b) => java_number(*b, format!("{b:e}"), b.to_string()),
        _ => return None,
    })
}

/// Text conversion of a Java real (`2.0`, `1.0E7`): numeric parameters of translations.
fn java_number(v: f64, sci: String, plain: String) -> String {
    if !v.is_finite() {
        return if v.is_nan() {
            "NaN".into()
        } else if v > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    if (1e-3..1e7).contains(&v.abs()) {
        return if plain.contains('.') { plain } else { format!("{plain}.0") };
    }
    let (m, e) = sci.split_once('e').unwrap_or((&sci, "0"));
    let m = if m.contains('.') { m.to_owned() } else { format!("{m}.0") };
    format!("{m}E{e}")
}

/// Component from its network NBT (§ 3.1); `None` if malformed.
pub fn from_nbt(v: &Nbt) -> Option<Component> {
    match v {
        Nbt::String(s) => Some(Component::text(s.clone())),
        Nbt::List(items) => {
            let mut it = items.iter();
            let mut first = from_nbt(it.next()?)?;
            for e in it {
                first.extra.push(from_nbt(e)?);
            }
            Some(first)
        }
        Nbt::Compound(fields) => {
            // Element of a heterogeneous list: `{"": x}` unwrapped.
            if fields.len() == 1 && fields[0].0.is_empty() {
                return from_nbt(&fields[0].1);
            }
            let get = |k: &str| v.get(k);
            let kind = get("type").and_then(Nbt::as_str);
            let content = if kind == Some("text") || (kind.is_none() && get("text").is_some()) {
                Content::Text(get("text").and_then(nbt_text).unwrap_or_default())
            } else if kind == Some("translatable") || (kind.is_none() && get("translate").is_some()) {
                let with = match get("with") {
                    Some(Nbt::List(a)) => a
                        .iter()
                        .map(|e| match e {
                            Nbt::Compound(_) | Nbt::List(_) => from_nbt(e).map(Arg::Component),
                            _ => nbt_text(e).map(Arg::Text),
                        })
                        .collect::<Option<Vec<_>>>()?,
                    _ => Vec::new(),
                };
                Content::Translate {
                    key: get("translate").and_then(Nbt::as_str)?.to_owned(),
                    fallback: get("fallback").and_then(Nbt::as_str).map(str::to_owned),
                    with,
                }
            } else if kind == Some("keybind") || (kind.is_none() && get("keybind").is_some()) {
                Content::Keybind(get("keybind").and_then(Nbt::as_str)?.to_owned())
            } else if kind == Some("selector") || (kind.is_none() && get("selector").is_some()) {
                Content::Selector(get("selector").and_then(Nbt::as_str)?.to_owned())
            } else if (kind == Some("object") || kind.is_none())
                && let Some(info) = object_info(v)
            {
                let fallback = match get("fallback") {
                    Some(f) => Some(Box::new(from_nbt(f)?)),
                    None => None,
                };
                Content::Object { info, fallback }
            } else if kind == Some("object") {
                return None;
            } else {
                Content::Nothing
            };
            let flag = |k: &str| get(k).and_then(nbt_bool);
            let color = match get("color").and_then(Nbt::as_str) {
                Some(c) => Some(parse_color(c)?),
                None => None,
            };
            let shadow_color = match get("shadow_color") {
                Some(Nbt::Int(i)) => Some(*i as u32),
                Some(Nbt::List(l)) if l.len() == 4 => {
                    let f = |i: usize| match &l[i] {
                        Nbt::Float(f) => (*f * 255.0).floor() as u32,
                        Nbt::Double(f) => (*f * 255.0).floor() as u32,
                        _ => 0,
                    };
                    Some(f(3) << 24 | f(0) << 16 | f(1) << 8 | f(2))
                }
                _ => None,
            };
            let style = Style {
                color,
                shadow_color,
                bold: flag("bold"),
                italic: flag("italic"),
                underlined: flag("underlined"),
                strikethrough: flag("strikethrough"),
                obfuscated: flag("obfuscated"),
                font: get("font").and_then(Nbt::as_str).map(str::to_owned),
                click: get("click_event").and_then(click_event),
                hover: get("hover_event").and_then(hover_event),
                insertion: get("insertion").and_then(Nbt::as_str).map(Arc::from),
            };
            let extra = match get("extra") {
                Some(Nbt::List(l)) => l.iter().map(from_nbt).collect::<Option<Vec<_>>>()?,
                _ => Vec::new(),
            };
            Some(Component { content, style, extra })
        }
        _ => nbt_text(v).map(Component::text),
    }
}

/// `object` content: `object` gives the kind (`atlas`, `player`), otherwise guessed from the fields
/// present (`sprite`, then `player`).
fn object_info(v: &Nbt) -> Option<ObjectInfo> {
    let atlas = || {
        let sprite = full_id(v.get("sprite")?.as_str()?);
        let atlas = v.get("atlas").and_then(Nbt::as_str).map_or_else(|| BLOCKS_ATLAS.to_owned(), full_id);
        Some(ObjectInfo::Atlas { atlas, sprite })
    };
    let player = || {
        let p = canonical_profile(v.get("player")?)?;
        let hat = v.get("hat").and_then(nbt_bool).unwrap_or(true);
        Some(ObjectInfo::Player { profile: Box::new(p), hat })
    };
    match v.get("object").and_then(Nbt::as_str) {
        Some("atlas") => atlas(),
        Some("player") => player(),
        Some(_) => None,
        None => atlas().or_else(player),
    }
}

/// Character allowed in chat (no `§`, no control, no DEL).
pub fn allowed_chat_char(c: char) -> bool {
    c != '§' && c >= ' ' && c != '\u{7f}'
}

/// Accepted address of a server: scheme `http` or `https`, no whitespace (§ 3.6).
pub fn valid_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else { return false };
    matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https")
        && !rest.is_empty()
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// `click_event` (§ 3.6): actions allowed coming from a server, fields validated; otherwise `None`
/// (the style keeps the rest, tolerated difference: the game would refuse the whole component). Also serves for the
/// fixed actions of the server's dialogs (same fields).
pub fn click_event(v: &Nbt) -> Option<Arc<ClickEvent>> {
    let action = v.get("action")?.as_str()?.to_owned();
    let text = |k: &str| v.get(k).and_then(Nbt::as_str).map(str::to_owned);
    let (value, nbt) = match action.as_str() {
        "open_url" => (text("url").filter(|u| valid_url(u))?, None),
        "run_command" | "suggest_command" => (text("command").filter(|c| c.chars().all(allowed_chat_char))?, None),
        "change_page" => {
            let p = v.get("page")?.as_i64().filter(|&p| p > 0)?;
            (p.to_string(), None)
        }
        "copy_to_clipboard" => (text("value")?, None),
        "show_dialog" => {
            let d = v.get("dialog")?.clone();
            (d.as_str().unwrap_or_default().to_owned(), Some(d))
        }
        "custom" => (text("id")?, v.get("payload").cloned()),
        // `open_file` is not allowed from a server.
        _ => return None,
    };
    Some(Arc::new(ClickEvent { action, value, nbt }))
}

/// UUID in NBT: array of four integers, or text `8-4-4-4-12`.
/// Nickname accepted by the game: 16 UTF-16 units at most, characters from 33 to 126.
fn valid_player_name(n: &str) -> bool {
    n.encode_utf16().count() <= 16 && n.chars().all(|c| ('!'..='~').contains(&c))
}

/// Valid resource identifier (optional namespace); returned complete.
fn resource_id(s: &str) -> Option<String> {
    let (ns, path) = s.split_once(':').unwrap_or(("minecraft", s));
    let ok = |s: &str, slash: bool| s.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-') || (slash && b == b'/'));
    (ok(ns, false) && ok(path, true)).then(|| format!("{}:{path}", if ns.is_empty() { "minecraft" } else { ns }))
}

/// Sequence of 4 integers (array of ints, bytes, longs, or list of numbers), like the game's
/// identifier reading; a string is not one.
fn int4(v: &Nbt) -> Option<Vec<i32>> {
    let n: Vec<i32> = match v {
        Nbt::IntArray(a) => a.clone(),
        Nbt::ByteArray(a) => a.iter().map(|&b| i32::from(b as i8)).collect(),
        Nbt::LongArray(a) => a.iter().map(|&l| l as i32).collect(),
        Nbt::List(l) => l
            .iter()
            .map(|e| match e {
                Nbt::Byte(b) => Some(i32::from(*b)),
                Nbt::Short(s) => Some(i32::from(*s)),
                Nbt::Int(i) => Some(*i),
                Nbt::Long(l) => Some(*l as i32),
                Nbt::Float(f) => Some(*f as i32),
                Nbt::Double(d) => Some(*d as i32),
                _ => None,
            })
            .collect::<Option<_>>()?,
        _ => return None,
    };
    (n.len() == 4).then_some(n)
}

/// Properties of a profile: list `{name, value, signature?}` or table `{name: [values]}` (16 at
/// most); rendered as a list, grouped by name in order of first appearance, like the game.
fn profile_properties(v: &Nbt) -> Option<Vec<Nbt>> {
    let utf16 = |s: &str| s.encode_utf16().count();
    let mut props: Vec<(String, String, Option<String>)> = Vec::new();
    match v {
        Nbt::Compound(map) => {
            if map.len() > 16 {
                return None;
            }
            for (name, values) in map {
                let Nbt::List(values) = values else { return None };
                for val in values {
                    props.push((name.clone(), val.as_str()?.to_owned(), None));
                }
            }
        }
        Nbt::List(l) => {
            if l.len() > 16 {
                return None;
            }
            for e in l {
                let name = e.get("name")?.as_str()?;
                let value = e.get("value")?.as_str()?;
                let signature = match e.get("signature") {
                    None => None,
                    Some(s) => Some(s.as_str().filter(|s| utf16(s) <= 1024)?.to_owned()),
                };
                if utf16(name) > 64 || utf16(value) > 32767 {
                    return None;
                }
                props.push((name.to_owned(), value.to_owned(), signature));
            }
        }
        _ => return None,
    }
    let mut names: Vec<&str> = Vec::new();
    for (n, ..) in &props {
        if !names.contains(&n.as_str()) {
            names.push(n);
        }
    }
    Some(
        names
            .iter()
            .flat_map(|n| props.iter().filter(move |p| p.0 == *n))
            .map(|(name, value, signature)| {
                let mut f = vec![("name".to_owned(), Nbt::String(name.clone())), ("value".to_owned(), Nbt::String(value.clone()))];
                if let Some(s) = signature {
                    f.push(("signature".to_owned(), Nbt::String(s.clone())));
                }
                Nbt::Compound(f)
            })
            .collect(),
    )
}

/// Profile of a head in the text re-read then rewritten like the game: bare nickname (valid string),
/// or compound where every known field present must be valid (otherwise the component is refused):
/// `name`, `id` (4 integers, rendered as an array; a string is refused), `properties`, then the
/// imposed skin `texture`, `cape`, `elytra` (full identifiers) and `model` (`wide`, `slim`).
/// Unknown fields removed; name and identifier both present: `id` first.
fn canonical_profile(p: &Nbt) -> Option<Nbt> {
    let Nbt::Compound(_) = p else {
        return p.as_str().filter(|n| valid_player_name(n)).map(|_| p.clone());
    };
    let name = match p.get("name") {
        None => None,
        Some(n) => Some(n.as_str().filter(|n| valid_player_name(n))?.to_owned()),
    };
    let id = match p.get("id") {
        None => None,
        Some(v) => Some(Nbt::IntArray(int4(v)?)),
    };
    let properties = match p.get("properties") {
        None => Vec::new(),
        Some(v) => profile_properties(v)?,
    };
    let mut out: Vec<(String, Nbt)> = Vec::new();
    let name = name.map(|n| ("name".to_owned(), Nbt::String(n)));
    let id = id.map(|i| ("id".to_owned(), i));
    match (name, id) {
        (Some(n), Some(i)) => out.extend([i, n]),
        (n, i) => out.extend(n.into_iter().chain(i)),
    }
    if !properties.is_empty() {
        out.push(("properties".to_owned(), Nbt::List(properties)));
    }
    for k in ["texture", "cape", "elytra"] {
        if let Some(t) = p.get(k) {
            out.push((k.to_owned(), Nbt::String(resource_id(t.as_str()?)?)));
        }
    }
    if let Some(m) = p.get("model") {
        let m = m.as_str().filter(|m| matches!(*m, "wide" | "slim"))?;
        out.push(("model".to_owned(), Nbt::String(m.to_owned())));
    }
    Some(Nbt::Compound(out))
}

fn nbt_uuid(v: &Nbt) -> Option<u128> {
    match v {
        Nbt::IntArray(a) if a.len() == 4 => Some(a.iter().fold(0u128, |acc, &i| acc << 32 | u128::from(i as u32))),
        Nbt::String(s) => {
            let hex: String = s.chars().filter(|&c| c != '-').collect();
            (hex.len() == 32).then(|| u128::from_str_radix(&hex, 16).ok()).flatten()
        }
        _ => None,
    }
}

/// `hover_event` (§ 3.6): text, item or entity.
fn hover_event(v: &Nbt) -> Option<Arc<HoverEvent>> {
    let h = match v.get("action")?.as_str()? {
        "show_text" => HoverEvent::Text(from_nbt(v.get("value")?)?),
        "show_item" => HoverEvent::Item {
            id: v.get("id")?.as_str()?.to_owned(),
            count: v.get("count").and_then(Nbt::as_i64).map_or(1, |c| c as i32),
            components: v.get("components").cloned(),
        },
        "show_entity" => HoverEvent::Entity {
            id: v.get("id")?.as_str()?.to_owned(),
            uuid: nbt_uuid(v.get("uuid")?)?,
            name: v.get("name").and_then(from_nbt),
        },
        _ => return None,
    };
    Some(Arc::new(h))
}

/// Plain text of a sequence of segments (§ codes included).
pub fn plain_text(segments: &[Segment]) -> String {
    segments.iter().map(|s| s.text.as_str()).collect()
}

/// Valid § codes removed (`§[0-9a-fk-or]`, case-insensitive); the other `§` stay.
pub fn strip_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        let code = |n: &char| matches!(n.to_ascii_lowercase(), '0'..='9' | 'a'..='f' | 'k'..='o' | 'r');
        if c == '§' && it.peek().is_some_and(code) {
            it.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Translation table (§ 3.5).
#[derive(Debug, Default, Clone)]
pub struct Lang {
    pub map: HashMap<String, String>,
    /// Right-to-left language (the `bidirectional` flag of its declaration): default direction of
    /// paragraphs without a strong letter (bidi).
    pub bidirectional: bool,
}

impl Lang {
    /// `en_us.json`, then the chosen language over it, then `deprecated.json`.
    pub fn load(base: &[u8], chosen: Option<&[u8]>, deprecated: Option<&[u8]>) -> Lang {
        let mut map: HashMap<String, String> = HashMap::new();
        for src in std::iter::once(base).chain(chosen) {
            if let Ok(serde_json::Value::Object(o)) = serde_json::from_slice::<serde_json::Value>(src) {
                for (k, v) in o {
                    if let Some(s) = v.as_str() {
                        map.insert(k, s.to_owned());
                    }
                }
            }
        }
        if let Some(Ok(d)) = deprecated.map(serde_json::from_slice::<serde_json::Value>) {
            for k in d["removed"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
                map.remove(k);
            }
            for (old, new) in d["renamed"].as_object().into_iter().flatten() {
                let Some(new) = new.as_str() else { continue };
                match map.remove(old) {
                    Some(v) => {
                        map.insert(new.to_owned(), v);
                    }
                    None => {
                        map.remove(new);
                    }
                }
            }
        }
        for v in map.values_mut() {
            *v = normalize_formats(v);
        }
        Lang { map, bidirectional: false }
    }
}

/// `%[n$][digits/points]d` and `…f` rewritten as `%[n$]s`.
fn normalize_formats(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == '%' {
            let mut j = i + 1;
            let start_num = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let pos = if j < b.len() && b[j] == '$' && j > start_num { Some(j) } else { None };
            let mut k = pos.map_or(i + 1, |p| p + 1);
            while k < b.len() && (b[k].is_ascii_digit() || b[k] == '.') {
                k += 1;
            }
            if k < b.len() && (b[k] == 'd' || b[k] == 'f') {
                out.push('%');
                if let Some(p) = pos {
                    out.extend(&b[i + 1..=p]);
                }
                out.push('s');
                i = k + 1;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Piece of a translation template.
enum Part {
    Lit(String),
    Arg(usize),
}

/// Splitting of a template; `None` → show the template as is.
fn split_template(t: &str, nargs: usize) -> Option<Vec<Part>> {
    let b: Vec<char> = t.chars().collect();
    let (mut parts, mut lit, mut next) = (Vec::new(), String::new(), 0usize);
    let mut i = 0;
    while i < b.len() {
        if b[i] != '%' {
            lit.push(b[i]);
            i += 1;
            continue;
        }
        if b.get(i + 1) == Some(&'%') {
            lit.push('%');
            i += 2;
            continue;
        }
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let index = if j > i + 1 && b.get(j) == Some(&'$') && b.get(j + 1) == Some(&'s') {
            let n: usize = b[i + 1..j].iter().collect::<String>().parse().ok()?;
            i = j + 2;
            n.checked_sub(1)?
        } else if b.get(i + 1) == Some(&'s') {
            i += 2;
            next += 1;
            next - 1
        } else {
            return None;
        };
        if index >= nargs {
            return None;
        }
        if !lit.is_empty() {
            parts.push(Part::Lit(std::mem::take(&mut lit)));
        }
        parts.push(Part::Arg(index));
    }
    if !lit.is_empty() {
        parts.push(Part::Lit(lit));
    }
    Some(parts)
}

/// Piece of a template split without copying: borrowed literal (copied only if it contains `%%`).
enum PartRef<'a> {
    Lit(std::borrow::Cow<'a, str>),
    Arg(usize),
}

/// Like [`split_template`], on bytes (the markers are ASCII) and without copying the template
/// (`RUSTCRAFT_OPT_LANGPARTS`): same pieces, same refusals.
fn split_template_ref(t: &str, nargs: usize) -> Option<Vec<PartRef<'_>>> {
    use std::borrow::Cow;
    let b = t.as_bytes();
    let mut parts = Vec::new();
    // Literal in progress: text already gathered (after a `%%`) then `t[run..i]`.
    let mut acc: Option<String> = None;
    let mut run = 0;
    let mut next = 0usize;
    let mut i = 0;
    fn take<'a>(t: &'a str, acc: &mut Option<String>, run: usize, end: usize) -> Cow<'a, str> {
        match acc.take() {
            None => Cow::Borrowed(&t[run..end]),
            Some(mut s) => {
                s.push_str(&t[run..end]);
                Cow::Owned(s)
            }
        }
    }
    while i < b.len() {
        if b[i] != b'%' {
            i += 1;
            continue;
        }
        if b.get(i + 1) == Some(&b'%') {
            acc.get_or_insert_with(String::new).push_str(&t[run..=i]);
            i += 2;
            run = i;
            continue;
        }
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let at = i;
        let index = if j > i + 1 && b.get(j) == Some(&b'$') && b.get(j + 1) == Some(&b's') {
            let n: usize = t[i + 1..j].parse().ok()?;
            i = j + 2;
            n.checked_sub(1)?
        } else if b.get(i + 1) == Some(&b's') {
            i += 2;
            next += 1;
            next - 1
        } else {
            return None;
        };
        if index >= nargs {
            return None;
        }
        let lit = take(t, &mut acc, run, at);
        if !lit.is_empty() {
            parts.push(PartRef::Lit(lit));
        }
        parts.push(PartRef::Arg(index));
        run = i;
    }
    let lit = take(t, &mut acc, run, b.len());
    if !lit.is_empty() {
        parts.push(PartRef::Lit(lit));
    }
    Some(parts)
}

/// `RUSTCRAFT_OPT_LANGPARTS` (default): translation templates split without copying.
fn lang_parts() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_LANGPARTS") != Some("0")
}

/// Flattened segment: inherited style and text (§ codes still present).
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub style: Style,
    pub text: String,
}

/// Flattens a component for drawing (§ 3.3): content then children, inherited styles,
/// translations resolved; an object becomes a carrier character in its reserved font.
pub fn flatten(c: &Component, lang: &Lang) -> Vec<Segment> {
    let mut out = Vec::new();
    walk(c, &Style::default(), lang, Walk { plain: false, parts: lang_parts() }, &mut out);
    out
}

/// Way of flattening: plain text (objects replaced by their fallback), templates split without copying.
#[derive(Clone, Copy)]
struct Walk {
    plain: bool,
    parts: bool,
}

/// Plain text of a component (copy, search, comparisons): an object is worth its fallback
/// text there, as in the game.
pub fn plain_string(c: &Component, lang: &Lang) -> String {
    let mut out = Vec::new();
    walk(c, &Style::default(), lang, Walk { plain: true, parts: lang_parts() }, &mut out);
    plain_text(&out)
}

fn walk(c: &Component, parent: &Style, lang: &Lang, mode: Walk, out: &mut Vec<Segment>) {
    let plain = mode.plain;
    let style = c.style.inherit(parent);
    let push = |out: &mut Vec<Segment>, s: &str, st: &Style| {
        if !s.is_empty() {
            out.push(Segment { style: st.clone(), text: s.to_owned() });
        }
    };
    match &c.content {
        Content::Text(s) | Content::Selector(s) => push(out, s, &style),
        Content::Keybind(action) => push(out, &keybind_text(lang, action), &style),
        Content::Object { info, fallback } => {
            if plain {
                match fallback {
                    Some(f) => walk(f, &style, lang, mode, out),
                    None => push(out, &info.default_fallback(), &style),
                }
            } else {
                let mut st = style.clone();
                st.font = Some(info.font());
                push(out, &OBJECT_CHAR.to_string(), &st);
            }
        }
        Content::Translate { key, fallback, with } if mode.parts => {
            let template: &str = lang.map.get(key).or(fallback.as_ref()).unwrap_or(key);
            match split_template_ref(template, with.len()) {
                Some(parts) => {
                    for p in parts {
                        match p {
                            PartRef::Lit(s) => push(out, &s, &style),
                            PartRef::Arg(i) => match &with[i] {
                                Arg::Component(a) => walk(a, &style, lang, mode, out),
                                Arg::Text(s) => push(out, s, &style),
                            },
                        }
                    }
                }
                None => push(out, template, &style),
            }
        }
        Content::Translate { key, fallback, with } => {
            let template = lang.map.get(key).or(fallback.as_ref()).cloned().unwrap_or_else(|| key.clone());
            match split_template(&template, with.len()) {
                Some(parts) => {
                    for p in parts {
                        match p {
                            Part::Lit(s) => push(out, &s, &style),
                            Part::Arg(i) => match &with[i] {
                                Arg::Component(a) => walk(a, &style, lang, mode, out),
                                Arg::Text(s) => push(out, s, &style),
                            },
                        }
                    }
                }
                None => push(out, &template, &style),
            }
        }
        Content::Nothing => {}
    }
    for e in &c.extra {
        walk(e, &style, lang, mode, out);
    }
}

/// Styled character after applying the § codes of its segment (§ 3.4).
#[derive(Debug, Clone, PartialEq)]
pub struct StyledChar {
    pub c: char,
    pub style: CharStyle,
}

/// Styled characters of a sequence of segments; the § state does not spill from one segment to the next.
pub fn styled_chars(segments: &[Segment]) -> Vec<StyledChar> {
    let mut out = Vec::new();
    for seg in segments {
        let base = CharStyle::from(&seg.style);
        let mut cur = base.clone();
        let mut it = seg.text.chars();
        while let Some(c) = it.next() {
            if c != '§' {
                out.push(StyledChar { c, style: cur.clone() });
                continue;
            }
            let Some(code) = it.next() else { break };
            match code.to_ascii_lowercase() {
                d @ ('0'..='9' | 'a'..='f') => {
                    let i = d.to_digit(16).unwrap_or(0) as usize;
                    cur = CharStyle {
                        color: Some(COLORS[i].1),
                        font: cur.font.clone(),
                        shadow_color: cur.shadow_color,
                        click: cur.click.clone(),
                        hover: cur.hover.clone(),
                        insertion: cur.insertion.clone(),
                        ..CharStyle::default()
                    };
                }
                'k' => cur.obfuscated = true,
                'l' => cur.bold = true,
                'm' => cur.strikethrough = true,
                'n' => cur.underlined = true,
                'o' => cur.italic = true,
                'r' => cur = base.clone(),
                _ => {}
            }
        }
    }
    out
}

/// Advance of a character in its font (without bold) and bold offset of the glyph.
pub trait Metrics {
    fn advance(&self, font: &str, c: char) -> (f32, f32);
}

fn char_advance(m: &dyn Metrics, s: &StyledChar) -> f32 {
    let (a, bold) = m.advance(&s.style.font, s.c);
    if s.style.bold { a + bold } else { a }
}

/// Width of a line: `ceil(sum of advances)` (§ 4).
pub fn width(m: &dyn Metrics, chars: &[StyledChar]) -> i32 {
    chars.iter().map(|s| char_advance(m, s)).sum::<f32>().ceil() as i32
}

/// Number of leading characters that fit in `max`: the longest prefix whose width
/// (`width`) does not exceed `max`. A single pass, advances accumulated in the same order as
/// `width` (same result as re-measuring each prefix).
pub fn fit(m: &dyn Metrics, chars: &[StyledChar], max: i32) -> usize {
    let mut acc = 0.0f32;
    for (i, s) in chars.iter().enumerate() {
        acc += char_advance(m, s);
        if acc.ceil() as i32 > max {
            return i;
        }
    }
    chars.len()
}

/// Text cut to `max` wide, ended with "..." if it overflows (the cut leaves room for the
/// dots).
pub fn clamp(m: &dyn Metrics, chars: &[StyledChar], max: i32) -> Vec<StyledChar> {
    if width(m, chars) <= max {
        return chars.to_vec();
    }
    let dots: Vec<StyledChar> = "...".chars().map(|c| StyledChar { c, style: CharStyle::default() }).collect();
    let n = fit(m, chars, max - width(m, &dots));
    let mut out = chars[..n].to_vec();
    out.extend(dots);
    out
}

/// Splitting into lines at width `max` (§ 4, rules 1–6).
pub fn wrap(m: &dyn Metrics, chars: &[StyledChar], max: f32) -> Vec<Vec<StyledChar>> {
    let max = max.max(1.0);
    let mut lines = Vec::new();
    let mut rest: &[StyledChar] = chars;
    loop {
        let mut acc = 0.0f32;
        let mut last_space: Option<usize> = None;
        let mut accepted_visible = false;
        let mut cut: Option<usize> = None;
        for (i, s) in rest.iter().enumerate() {
            if s.c == '\n' {
                cut = Some(i);
                break;
            }
            if s.c == ' ' {
                last_space = Some(i);
            }
            let a = char_advance(m, s);
            acc += a;
            if acc > max && accepted_visible {
                cut = Some(last_space.unwrap_or(i));
                break;
            }
            if a != 0.0 {
                accepted_visible = true;
            }
        }
        match cut {
            None => {
                lines.push(rest.to_vec());
                return lines;
            }
            Some(i) => {
                lines.push(rest[..i].to_vec());
                let skip = usize::from(matches!(rest.get(i).map(|s| s.c), Some(' ' | '\n')));
                rest = &rest[i + skip..];
                if rest.is_empty() && skip == 1 && chars.last().is_some_and(|s| s.c == '\n') {
                    lines.push(Vec::new());
                    return lines;
                }
                if rest.is_empty() {
                    return lines;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(fields: &[(&str, Nbt)]) -> Nbt {
        Nbt::Compound(fields.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect())
    }

    fn lang(pairs: &[(&str, &str)]) -> Lang {
        Lang { map: pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(), bidirectional: false }
    }

    /// Fake advances: "i" 2.5, "." 2, the rest 6 (bold + 1).
    struct Fake;
    impl Metrics for Fake {
        fn advance(&self, _font: &str, c: char) -> (f32, f32) {
            (
                match c {
                    'i' => 2.5,
                    '.' => 2.0,
                    _ => 6.0,
                },
                1.0,
            )
        }
    }

    fn sc(s: &str) -> Vec<StyledChar> {
        s.chars().map(|c| StyledChar { c, style: CharStyle::default() }).collect()
    }

    /// `fit` returns the longest prefix of width ≤ max, identical to a re-measure of each
    /// prefix; `clamp` leaves room for the "...".
    #[test]
    fn fit_matches_prefix_widths_and_clamp_adds_dots() {
        let text = sc("aiiaiaiiiaa");
        for max in -1..80 {
            let slow = (0..=text.len()).rev().find(|&n| width(&Fake, &text[..n]) <= max).unwrap_or(0);
            assert_eq!(fit(&Fake, &text, max), slow, "max {max}");
        }
        assert_eq!(text_of(&clamp(&Fake, &text, 200)), "aiiaiaiiiaa");
        // 30 wide: 24 for the text ("..." = 6), "aiia" = 17, "aiiai" = 19.5 → 20,
        // "aiiaia" = 25.5 → 26 > 24.
        assert_eq!(text_of(&clamp(&Fake, &text, 30)), "aiiai...");
    }

    /// Bench (`--ignored`): "..." cut of a 64-character name at 157, old method (width
    /// re-measured at each added character) against `fit` (one pass). Figures in `docs/perf.md`.
    #[test]
    #[ignore]
    fn clamp_bench() {
        let text = sc(&"aiiaiaiiiaaiaiaiiaia".repeat(4)[..64]);
        let old = |s: &[StyledChar], max: i32| {
            let dots = sc("...");
            let room = max - width(&Fake, &dots);
            let mut out = Vec::new();
            for c in s {
                out.push(c.clone());
                if width(&Fake, &out) > room {
                    out.pop();
                    break;
                }
            }
            out.extend(dots);
            out
        };
        assert_eq!(old(&text, 157), clamp(&Fake, &text, 157));
        let n = 200_000;
        for (name, f) in [("old", &old as &dyn Fn(&[StyledChar], i32) -> Vec<StyledChar>), ("fit", &|s: &[StyledChar], m: i32| clamp(&Fake, s, m))] {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                for _ in 0..n {
                    std::hint::black_box(f(std::hint::black_box(&text), 157));
                }
                best = best.min(t.elapsed().as_secs_f64() * 1e9 / n as f64);
            }
            eprintln!("clamp {name}: {best:.0} ns per call (best of 5)");
        }
    }

    fn text_of(v: &[StyledChar]) -> String {
        v.iter().map(|c| c.c).collect()
    }

    /// Frozen pseudo-random generator (xorshift) for the property proofs.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// Random template rich in markers (valid, invalid, escaped, positional).
    fn template(r: &mut Rng) -> String {
        const BITS: [&str; 14] =
            ["a", "é", " ", "%", "%%", "%s", "%1$s", "%2$s", "%3$s", "%0$s", "$", "1", "%d", "%99999999999999999999$s"];
        (0..r.next(8)).map(|_| BITS[r.next(BITS.len() as u64) as usize]).collect()
    }

    fn component(r: &mut Rng, depth: u32) -> Component {
        let mut c = match r.next(3) {
            0 => Component::text(template(r)),
            _ => {
                let key = ["k.a", "k.b", "k.c", "absent"][r.next(4) as usize];
                let with = (0..r.next(4))
                    .map(|_| {
                        if depth > 0 && r.next(2) == 0 {
                            Arg::Component(component(r, depth - 1))
                        } else {
                            Arg::Text(template(r))
                        }
                    })
                    .collect();
                let mut c = Component::translate(key, with);
                if r.next(3) == 0
                    && let Content::Translate { fallback, .. } = &mut c.content
                {
                    *fallback = Some(template(r));
                }
                c
            }
        };
        if r.next(2) == 0 {
            c.style.color = Some(r.next(0x100_0000) as u32);
        }
        if r.next(3) == 0 {
            c.style.bold = Some(r.next(2) == 0);
        }
        if r.next(4) == 0 {
            c.style.font = Some("minecraft:alt".into());
        }
        if depth > 0 {
            c.extra = (0..r.next(3)).map(|_| component(r, depth - 1)).collect();
        }
        c
    }

    /// Micro-bench (loaded machine possible): flattening of a translated chat message and of an
    /// enchanted tooltip line, old splitting against copy-free splitting.
    /// `cargo test --release -p rc-text flatten_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn flatten_bench() {
        let l = lang(&[
            ("chat.type.text", "<%s> %s"),
            ("enchantment.minecraft.sharpness", "Sharpness"),
            ("enchantment.level.5", "V"),
            ("potion.withAmplifier", "%s %s"),
        ]);
        let mut name = Component::text("Player");
        name.style.color = Some(0xffff55);
        let chat = Component::translate(
            "chat.type.text",
            vec![Arg::Component(name), Arg::Text("hello everyone, anyone got iron?".into())],
        );
        let ench = Component::translate(
            "potion.withAmplifier",
            vec![
                Arg::Component(Component::translate("enchantment.minecraft.sharpness", vec![])),
                Arg::Component(Component::translate("enchantment.level.5", vec![])),
            ],
        );
        for (label, c) in [("chat", &chat), ("tooltip", &ench)] {
            for parts in [false, true] {
                const N: u32 = 200_000;
                let t = std::time::Instant::now();
                for _ in 0..N {
                    let mut out = Vec::new();
                    walk(std::hint::black_box(c), &Style::default(), &l, Walk { plain: false, parts }, &mut out);
                    std::hint::black_box(styled_chars(&out));
                }
                let ns = t.elapsed().as_secs_f64() * 1e9 / f64::from(N);
                eprintln!("{label:<10} {:<14} {ns:>7.0} ns", if parts { "copy-free" } else { "old" });
            }
        }
    }

    /// `RUSTCRAFT_OPT_LANGPARTS`: 10,000 random components flattened (drawing and plain text)
    /// by copy-free splitting give the same segments and styled characters as the old
    /// splitting, and both splittings return the same pieces for 20,000 templates.
    #[test]
    fn template_parts_match_old_split() {
        let mut r = Rng(0x9e37_79b9_7f4a_7c15);
        let mut rendered = 0;
        for _ in 0..20_000 {
            let t = template(&mut r);
            let n = r.next(4) as usize;
            let old = split_template(&t, n).map(|v| {
                v.into_iter()
                    .map(|p| match p {
                        Part::Lit(s) => (Some(s), None),
                        Part::Arg(i) => (None, Some(i)),
                    })
                    .collect::<Vec<_>>()
            });
            let new = split_template_ref(&t, n).map(|v| {
                v.into_iter()
                    .map(|p| match p {
                        PartRef::Lit(s) => (Some(s.into_owned()), None),
                        PartRef::Arg(i) => (None, Some(i)),
                    })
                    .collect::<Vec<_>>()
            });
            assert_eq!(old, new, "template {t:?}, {n} arguments");
            rendered += usize::from(new.is_some_and(|v| v.iter().any(|p| p.1.is_some())));
        }
        assert!(rendered > 1000, "too few templates with an argument ({rendered})");
        let l = lang(&[("k.a", "A %s and %2$s %%"), ("k.b", "%1$s%1$s"), ("k.c", "percent %d %")]);
        for _ in 0..10_000 {
            let c = component(&mut r, 3);
            for plain in [false, true] {
                let (mut old, mut new) = (Vec::new(), Vec::new());
                walk(&c, &Style::default(), &l, Walk { plain, parts: false }, &mut old);
                walk(&c, &Style::default(), &l, Walk { plain, parts: true }, &mut new);
                assert_eq!(old, new, "{c:?}");
                assert_eq!(styled_chars(&old), styled_chars(&new));
            }
        }
    }

    #[test]
    fn keybind_shows_the_bound_key_or_the_action_name() {
        let l = lang(&[("key.keyboard.space", "Space"), ("key.sneak", "Sneak"), ("key.mouse", "Button %1$s")]);
        set_keybinds([
            ("key.jump".to_owned(), "key.keyboard.space".to_owned()),
            ("key.forward".to_owned(), "key.keyboard.w".to_owned()),
            ("key.pickItem".to_owned(), "key.mouse.4".to_owned()),
        ]);
        let text = |k: &str| {
            let c = from_nbt(&obj(&[("keybind", Nbt::String(k.into()))])).unwrap();
            plain_text(&flatten(&c, &l))
        };
        assert_eq!(text("key.jump"), "Space");
        assert_eq!(text("key.forward"), "W");
        assert_eq!(text("key.pickItem"), "Button 4");
        // Unknown key action: translation of its name, otherwise the raw name.
        assert_eq!(text("key.sneak"), "Sneak");
        assert_eq!(text("key.unknown"), "key.unknown");
    }

    #[test]
    fn object_contents_parse_and_fall_back() {
        let l = Lang::default();
        let atlas = from_nbt(&obj(&[("sprite", Nbt::String("item/diamond".into()))])).unwrap();
        assert_eq!(
            atlas.content,
            Content::Object {
                info: ObjectInfo::Atlas { atlas: "minecraft:blocks".into(), sprite: "minecraft:item/diamond".into() },
                fallback: None
            }
        );
        assert_eq!(plain_string(&atlas, &l), "[item/diamond]");
        let other = from_nbt(&obj(&[
            ("type", Nbt::String("object".into())),
            ("object", Nbt::String("atlas".into())),
            ("atlas", Nbt::String("gui".into())),
            ("sprite", Nbt::String("icon/ping_1".into())),
        ]))
        .unwrap();
        assert_eq!(plain_string(&other, &l), "[icon/ping_1@gui]");
        let head = from_nbt(&obj(&[
            ("player", obj(&[("name", Nbt::String("Steve".into()))])),
            ("fallback", Nbt::String("head".into())),
        ]))
        .unwrap();
        assert_eq!(plain_string(&head, &l), "head");
        let anon = from_nbt(&obj(&[("object", Nbt::String("player".into())), ("player", obj(&[]))])).unwrap();
        assert_eq!(plain_string(&anon, &l), "[unknown player head]");
        // For drawing, any object is a carrier character in its reserved font (the game
        // draws the sprite, or the missing texture, never the fallback text).
        for (c, want) in [
            (&atlas, ObjectFont::Atlas { atlas: "minecraft:blocks", sprite: "minecraft:item/diamond" }),
            (&other, ObjectFont::Atlas { atlas: "minecraft:gui", sprite: "minecraft:icon/ping_1" }),
            (&head, ObjectFont::Player { hat: true, uuid: Err("Steve"), request: Some((head_hash("name:steve"), "name:Steve")), body: "" }),
            (&anon, ObjectFont::Player { hat: true, uuid: Ok(0), request: None, body: "" }),
        ] {
            let seg = flatten(c, &l);
            assert_eq!(seg[0].text, OBJECT_CHAR.to_string());
            assert_eq!(object_font(seg[0].style.font.as_deref().unwrap()), Some(want));
        }
    }

    #[test]
    fn player_object_profile_fields() {
        let ints = Nbt::IntArray(vec![0x0102_0304, 0x0506_0708, 0x090a_0b0c, 0x0d0e_0f10]);
        let props = Nbt::List(vec![obj(&[("name", Nbt::String("textures".into())), ("value", Nbt::String("e30=".into()))])]);
        let c = from_nbt(&obj(&[
            ("player", obj(&[("id", ints), ("properties", props), ("texture", Nbt::String("entity/player/wide/alex".into()))])),
            ("hat", Nbt::Byte(0)),
        ]))
        .unwrap();
        let id = 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10u128;
        let seg = flatten(&c, &Lang::default());
        assert_eq!(
            object_font(seg[0].style.font.as_deref().unwrap()),
            Some(ObjectFont::Player {
                hat: false,
                uuid: Ok(id),
                request: Some((id, "e30=")),
                body: "minecraft:entity/player/wide/alex"
            })
        );
        // Identifier alone (list of numbers): profile resolved by the profile service.
        let list = Nbt::List([0x0102_0304, 0x0506_0708, 0x090a_0b0c, 0x0d0e_0f10].map(Nbt::Int).to_vec());
        let c = from_nbt(&obj(&[("player", obj(&[("id", list)]))])).unwrap();
        let seg = flatten(&c, &Lang::default());
        let Some(ObjectFont::Player { request, .. }) = object_font(seg[0].style.font.as_deref().unwrap()) else { panic!() };
        assert_eq!(request, Some((id, "id:0102030405060708090a0b0c0d0e0f10")));
    }

    #[test]
    fn player_object_profile_is_rewritten_like_the_game() {
        let profile = |c: &Component| match &c.content {
            Content::Object { info: ObjectInfo::Player { profile, .. }, .. } => (**profile).clone(),
            _ => panic!("not a head"),
        };
        let head = |p: Nbt| {
            from_nbt(&obj(&[("type", Nbt::String("object".into())), ("object", Nbt::String("player".into())), ("player", p)]))
        };
        let ints = vec![0x0102_0304, 0x0506_0708, 0x090a_0b0c, 0x0d0e_0f10];
        // Unknown fields removed, identifier as an array, the game's order (id, name, properties,
        // imposed skin), properties as a table rendered as a list.
        let c = head(obj(&[
            ("model", Nbt::String("slim".into())),
            ("foo", Nbt::Int(1)),
            ("texture", Nbt::String("entity/player/wide/alex".into())),
            ("properties", obj(&[("textures", Nbt::List(vec![Nbt::String("e30=".into())]))])),
            ("name", Nbt::String("Steve".into())),
            ("id", Nbt::List(ints.iter().copied().map(Nbt::Int).collect())),
        ]))
        .unwrap();
        let prop = obj(&[("name", Nbt::String("textures".into())), ("value", Nbt::String("e30=".into()))]);
        assert_eq!(
            profile(&c),
            obj(&[
                ("id", Nbt::IntArray(ints.clone())),
                ("name", Nbt::String("Steve".into())),
                ("properties", Nbt::List(vec![prop])),
                ("texture", Nbt::String("minecraft:entity/player/wide/alex".into())),
                ("model", Nbt::String("slim".into())),
            ])
        );
        // Name alone (without identifier): rendered without reordering.
        assert_eq!(profile(&head(obj(&[("name", Nbt::String("Alex".into()))])).unwrap()), obj(&[("name", Nbt::String("Alex".into()))]));
        // Refused as by the game: identifier as a string, name too long or forbidden, unknown
        // model, invalid bare nickname.
        assert!(head(obj(&[("id", Nbt::String("01020304-0506-0708-090a-0b0c0d0e0f10".into()))])).is_none());
        assert!(head(obj(&[("name", Nbt::String("NameMuchTooLongHere".into()))])).is_none());
        assert!(head(obj(&[("name", Nbt::String("a b".into()))])).is_none());
        assert!(head(obj(&[("model", Nbt::String("large".into()))])).is_none());
        assert!(head(Nbt::String("é".into())).is_none());
        assert!(head(Nbt::String("Steve".into())).is_some());
    }

    struct Fixed;
    impl Metrics for Fixed {
        fn advance(&self, _: &str, c: char) -> (f32, f32) {
            (if c == ' ' { 4.0 } else { 6.0 }, 1.0)
        }
    }

    fn plain(s: &str) -> Vec<StyledChar> {
        styled_chars(&[Segment { style: Style::default(), text: s.into() }])
    }

    #[test]
    fn translation_with_arguments() {
        let mut lang = Lang::default();
        lang.map.insert("chat.type.text".into(), "<%s> %s".into());
        lang.map.insert("two".into(), "%2$s then %1$s 100%%".into());
        let c = Component::translate("chat.type.text", vec![Arg::Text("Bob".into()), Arg::Text("hi".into())]);
        let text: String = flatten(&c, &lang).iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "<Bob> hi");
        let c = Component::translate("two", vec![Arg::Text("a".into()), Arg::Text("b".into())]);
        let text: String = flatten(&c, &lang).iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "b then a 100%");
        // Invalid template or key absent.
        let c = Component::translate("absent.key", vec![]);
        assert_eq!(flatten(&c, &lang)[0].text, "absent.key");
        assert_eq!(normalize_formats("%d and %2$d and %.2f"), "%s and %2$s and %s");
    }

    #[test]
    fn section_codes() {
        let c = plain("a§cb§lc§rd");
        assert_eq!(c.len(), 4);
        assert_eq!(c[1].style.color, Some(0xFF5555));
        assert!(c[2].style.bold && c[2].style.color == Some(0xFF5555));
        assert_eq!(c[3].style, CharStyle::default());
    }

    #[test]
    fn wrapping_rules() {
        // "aaa bbb" at 30 px: 3×6 + 4 = 22, then bbb overflows → cut at the space, removed.
        let lines = wrap(&Fixed, &plain("aaa bbb"), 30.0);
        let s: Vec<String> = lines.iter().map(|l| l.iter().map(|c| c.c).collect()).collect();
        assert_eq!(s, ["aaa", "bbb"]);
        // Word too long: cut right in the middle.
        let lines = wrap(&Fixed, &plain("aaaaaa"), 13.0);
        let s: Vec<String> = lines.iter().map(|l| l.iter().map(|c| c.c).collect()).collect();
        assert_eq!(s, ["aa", "aa", "aa"]);
        assert_eq!(width(&Fixed, &plain("ab c")), 22);
    }

    #[test]
    fn nbt_list_children_inherit() {
        let v = Nbt::List(vec![
            Nbt::Compound(vec![("text".into(), Nbt::String("A".into())), ("color".into(), Nbt::String("red".into()))]),
            Nbt::String("B".into()),
        ]);
        let c = from_nbt(&v).unwrap();
        let segs = flatten(&c, &Lang::default());
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[1].style.color, Some(0xFF5555));
    }

    fn compound(fields: Vec<(&str, Nbt)>) -> Nbt {
        Nbt::Compound(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    #[test]
    fn click_hover_insertion_inherited_and_validated() {
        let s = |x: &str| Nbt::String(x.into());
        let link = compound(vec![("action", s("open_url")), ("url", s("https://example.org/a"))]);
        let bad = compound(vec![("action", s("open_url")), ("url", s("file:///c:/x"))]);
        let file = compound(vec![("action", s("open_file")), ("path", s("x"))]);
        let tip = compound(vec![("action", s("show_text")), ("value", s("tooltip"))]);
        let ent = compound(vec![
            ("action", s("show_entity")),
            ("id", s("minecraft:pig")),
            ("uuid", Nbt::IntArray(vec![0, 1, 0, 2])),
        ]);
        let v = compound(vec![
            ("text", s("A")),
            ("click_event", link),
            ("hover_event", tip),
            ("insertion", s("ins")),
            ("extra", Nbt::List(vec![s("B§cC"), compound(vec![("text", s("D")), ("click_event", bad)])])),
        ]);
        let c = from_nbt(&v).unwrap();
        let chars = styled_chars(&flatten(&c, &Lang::default()));
        // A, B, C (§ code keeping click and hover), D (address refused: parent's click inherited).
        assert_eq!(chars.len(), 4);
        for ch in &chars {
            assert_eq!(ch.style.click.as_ref().unwrap().value, "https://example.org/a");
            assert_eq!(ch.style.insertion.as_deref(), Some("ins"));
            assert!(matches!(ch.style.hover.as_deref(), Some(HoverEvent::Text(_))));
        }
        assert!(click_event(&file).is_none());
        match hover_event(&ent).as_deref() {
            Some(HoverEvent::Entity { uuid, .. }) => assert_eq!(*uuid, (1u128 << 64) | 2),
            other => panic!("{other:?}"),
        }
        assert_eq!(strip_codes("a§cb§Lc§zd§"), "abc§zd§");
    }
}
