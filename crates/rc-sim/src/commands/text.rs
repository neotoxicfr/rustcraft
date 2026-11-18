//! Components of command feedback: translations, colours, numbers in Java format,
//! component → network NBT (titles, named items).

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use rc_text::{Arg, ClickEvent, Component, Content, HoverEvent, Style};

pub const RED: u32 = 0xFF5555;
pub const GRAY: u32 = 0xAAAAAA;
pub const GREEN: u32 = 0x55FF55;
pub const AQUA: u32 = 0x55FFFF;
pub const GOLD: u32 = 0xFFAA00;
pub const YELLOW: u32 = 0xFFFF55;

/// Value of a translation parameter.
pub enum A {
    S(String),
    C(Component),
}

impl From<&str> for A {
    fn from(s: &str) -> A {
        A::S(s.to_owned())
    }
}
impl From<String> for A {
    fn from(s: String) -> A {
        A::S(s)
    }
}
impl From<&String> for A {
    fn from(s: &String) -> A {
        A::S(s.clone())
    }
}
impl From<i32> for A {
    fn from(v: i32) -> A {
        A::S(v.to_string())
    }
}
impl From<i64> for A {
    fn from(v: i64) -> A {
        A::S(v.to_string())
    }
}
impl From<usize> for A {
    fn from(v: usize) -> A {
        A::S(v.to_string())
    }
}
impl From<Component> for A {
    fn from(c: Component) -> A {
        A::C(c)
    }
}

/// Translation with parameters.
pub fn tr(key: &str, args: Vec<A>) -> Component {
    Component::translate(
        key,
        args.into_iter()
            .map(|a| match a {
                A::S(s) => Arg::Text(s),
                A::C(c) => Arg::Component(c),
            })
            .collect(),
    )
}

#[macro_export]
macro_rules! tr {
    ($key:expr) => { $crate::commands::text::tr($key, Vec::new()) };
    ($key:expr, $($a:expr),+ $(,)?) => { $crate::commands::text::tr($key, vec![$($crate::commands::text::A::from($a)),+]) };
}

pub fn lit(s: impl Into<String>) -> Component {
    Component::text(s)
}

pub fn colored(mut c: Component, color: u32) -> Component {
    c.style.color = Some(color);
    c
}

/// Empty component with children.
pub fn join(parts: Vec<Component>) -> Component {
    let mut c = Component::text("");
    c.extra = parts;
    c
}

/// List "a, b, c" (grey `, ` separator).
pub fn list(parts: Vec<Component>) -> Component {
    list_sep(parts, colored(lit(", "), GRAY))
}

/// List with a given separator; a single element rendered as is, none = empty text.
pub fn list_sep(mut parts: Vec<Component>, sep: Component) -> Component {
    if parts.len() == 1 {
        return parts.pop().unwrap_or_else(|| lit(""));
    }
    let mut out = Vec::new();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.push(sep.clone());
        }
        out.push(p);
    }
    join(out)
}

/// Green copyable text between brackets.
pub fn copy_text(t: &str) -> Component {
    let mut c = colored(lit(t), GREEN);
    copy_click(&mut c, t.to_owned());
    hover_text(&mut c, tr("chat.copy.click", Vec::new()));
    c.style.insertion = Some(Arc::from(t));
    brackets(c)
}

/// Between green brackets, like clickable coordinates.
pub fn brackets(c: Component) -> Component {
    tr!("chat.square_brackets", c)
}

pub fn suggest_click(c: &mut Component, cmd: String) {
    c.style.click = Some(Arc::new(ClickEvent { action: "suggest_command".into(), value: cmd, nbt: None }));
}

pub fn copy_click(c: &mut Component, v: String) {
    c.style.click = Some(Arc::new(ClickEvent { action: "copy_to_clipboard".into(), value: v, nbt: None }));
}

pub fn hover_text(c: &mut Component, t: Component) {
    c.style.hover = Some(Arc::new(HoverEvent::Text(t)));
}

/// Reals written like the game (`1.0`, `0.5`, `1.0E7`, `1.0E-4`), shared with the parsing.
pub use rc_commands::javafmt::{jd, jf};

/// `String.format("%.2f")` (rounded to nearest, Java: half up).
pub fn f2(v: f64) -> String {
    format!("{v:.2}")
}

/// Component → network NBT (the game's compact form: bare string if unstyled text).
pub fn to_nbt(c: &Component) -> Nbt {
    let mut f: Vec<(String, Nbt)> = Vec::new();
    match &c.content {
        Content::Text(t) => {
            if c.extra.is_empty() && c.style == Style::default() {
                return Nbt::String(t.clone());
            }
            f.push(("text".into(), Nbt::String(t.clone())));
        }
        Content::Translate { key, fallback, with } => {
            f.push(("translate".into(), Nbt::String(key.clone())));
            if let Some(fb) = fallback {
                f.push(("fallback".into(), Nbt::String(fb.clone())));
            }
            if !with.is_empty() {
                let args = with
                    .iter()
                    .map(|a| match a {
                        Arg::Text(s) => Nbt::String(s.clone()),
                        Arg::Component(c) => match to_nbt(c) {
                            Nbt::String(s) => Nbt::Compound(vec![("text".into(), Nbt::String(s))]),
                            n => n,
                        },
                    })
                    .collect::<Vec<_>>();
                // Homogeneous list: all compounds (bare strings are wrapped).
                let args = args
                    .into_iter()
                    .map(|n| match n {
                        Nbt::String(s) => Nbt::Compound(vec![("text".into(), Nbt::String(s))]),
                        o => o,
                    })
                    .collect();
                f.push(("with".into(), Nbt::List(args)));
            }
        }
        Content::Keybind(k) => f.push(("keybind".into(), Nbt::String(k.clone()))),
        Content::Selector(s) => f.push(("selector".into(), Nbt::String(s.clone()))),
        Content::Object { info, fallback } => {
            match info {
                rc_text::ObjectInfo::Atlas { atlas, sprite } => {
                    f.push(("object".into(), Nbt::String("atlas".into())));
                    f.push(("atlas".into(), Nbt::String(atlas.clone())));
                    f.push(("sprite".into(), Nbt::String(sprite.clone())));
                }
                rc_text::ObjectInfo::Player { profile, hat } => {
                    // The game always rewrites the profile as a compound: a bare name becomes
                    // `{name}`; a compound arrives already rewritten on reading (known fields
                    // only, identifier as an array, the game's order: `rc_text`).
                    f.push(("object".into(), Nbt::String("player".into())));
                    let p = match &**profile {
                        Nbt::String(n) => Nbt::Compound(vec![("name".to_owned(), Nbt::String(n.clone()))]),
                        other => other.clone(),
                    };
                    f.push(("player".into(), p));
                    f.push(("hat".into(), Nbt::Byte(i8::from(*hat))));
                }
            }
            if let Some(fb) = fallback {
                f.push(("fallback".into(), to_nbt(fb)));
            }
        }
        Content::Nothing => f.push(("text".into(), Nbt::String(String::new()))),
    }
    style_nbt(&c.style, &mut f);
    if !c.extra.is_empty() {
        let ex = c
            .extra
            .iter()
            .map(|e| match to_nbt(e) {
                Nbt::String(s) => Nbt::Compound(vec![("text".into(), Nbt::String(s))]),
                n => n,
            })
            .collect();
        f.push(("extra".into(), Nbt::List(ex)));
    }
    Nbt::Compound(f)
}

fn color_name(c: u32) -> String {
    const NAMES: [(&str, u32); 16] = [
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
    NAMES.iter().find(|n| n.1 == c).map_or_else(|| format!("#{c:06X}"), |n| n.0.to_owned())
}

fn style_nbt(s: &Style, f: &mut Vec<(String, Nbt)>) {
    if let Some(c) = s.color {
        f.push(("color".into(), Nbt::String(color_name(c))));
    }
    if let Some(c) = s.shadow_color {
        f.push(("shadow_color".into(), Nbt::Int(c as i32)));
    }
    for (k, v) in [
        ("bold", s.bold),
        ("italic", s.italic),
        ("underlined", s.underlined),
        ("strikethrough", s.strikethrough),
        ("obfuscated", s.obfuscated),
    ] {
        if let Some(b) = v {
            f.push((k.into(), Nbt::Byte(i8::from(b))));
        }
    }
    if let Some(font) = &s.font {
        f.push(("font".into(), Nbt::String(font.clone())));
    }
    if let Some(ins) = &s.insertion {
        f.push(("insertion".into(), Nbt::String(ins.to_string())));
    }
    if let Some(c) = &s.click {
        let mut e = vec![("action".into(), Nbt::String(c.action.clone()))];
        let key = match c.action.as_str() {
            "open_url" => "url",
            "run_command" | "suggest_command" => "command",
            "copy_to_clipboard" => "value",
            "custom" => "id",
            _ => "value",
        };
        if c.action == "change_page" {
            e.push(("page".into(), Nbt::Int(c.value.parse().unwrap_or(1))));
        } else {
            e.push((key.into(), Nbt::String(c.value.clone())));
        }
        f.push(("click_event".into(), Nbt::Compound(e)));
    }
    if let Some(h) = &s.hover {
        let e = match &**h {
            HoverEvent::Text(t) => {
                vec![("action".into(), Nbt::String("show_text".into())), ("value".into(), to_nbt(t))]
            }
            HoverEvent::Item { id, count, components } => {
                let mut e = vec![
                    ("action".into(), Nbt::String("show_item".into())),
                    ("id".into(), Nbt::String(id.clone())),
                    ("count".into(), Nbt::Int(*count)),
                ];
                if let Some(c) = components {
                    e.push(("components".into(), c.clone()));
                }
                e
            }
            HoverEvent::Entity { id, uuid, name } => {
                let mut e = vec![
                    ("action".into(), Nbt::String("show_entity".into())),
                    ("id".into(), Nbt::String(id.clone())),
                    ("uuid".into(), crate::nbtio::uuid_nbt(*uuid)),
                ];
                if let Some(n) = name {
                    e.push(("name".into(), to_nbt(n)));
                }
                e
            }
        };
        f.push(("hover_event".into(), Nbt::Compound(e)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_numbers() {
        assert_eq!(jd(1.0), "1.0");
        assert_eq!(jd(0.5), "0.5");
        assert_eq!(jd(-12.25), "-12.25");
        assert_eq!(jd(1e7), "1.0E7");
        assert_eq!(jd(1.5e-4), "1.5E-4");
        assert_eq!(jf(20.0), "20.0");
        assert_eq!(jf(0.1), "0.1");
    }
}
