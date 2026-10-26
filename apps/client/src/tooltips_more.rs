//! Remaining tooltip sections: tropical fish (bucket), painting (title, author, dimensions),
//! sign text, suspicious stew (creative inventory), spawner. Same lines,
//! colors and order as the official game.

use std::cell::Cell;
use std::sync::RwLock;

use rc_proto::Reader;
use rc_proto::nbt::{self, Nbt};
use rc_proto::varint::read_varint;
use rc_text::{Arg, Component};

thread_local! {
    /// Tooltip of a creative inventory item (the game's "creative" flag).
    static CREATIVE: Cell<bool> = const { Cell::new(false) };
}

/// Sets the "creative" flag for the tooltips built afterwards on this thread.
pub fn set_creative(on: bool) {
    CREATIVE.with(|c| c.set(on));
}

fn creative() -> bool {
    CREATIVE.with(Cell::get)
}

/// Painting variant of the synchronized registry: title, author, width, height.
struct Painting {
    id: String,
    title: Option<Component>,
    author: Option<Component>,
    size: Option<(i32, i32)>,
}

static PAINTINGS: RwLock<Vec<Painting>> = RwLock::new(Vec::new());

/// Remembers the painting registry (called when the registry is received).
pub fn set_registry(name: &str, entries: &rc_client::RegistryEntries) {
    if name != "minecraft:painting_variant" {
        return;
    }
    let int = |v: &Nbt, k: &str| v.get(k).and_then(Nbt::as_i64).map(|v| v as i32);
    let list = entries
        .iter()
        .map(|(id, v)| match v {
            Some(v) => Painting {
                id: id.clone(),
                title: v.get("title").and_then(rc_text::from_nbt),
                author: v.get("author").and_then(rc_text::from_nbt),
                size: int(v, "width").zip(int(v, "height")),
            },
            // Base game variant sent without data (known pack): texts and sizes
            // of the game (language keys, sizes of the extracted data).
            None => Painting { id: id.clone(), title: None, author: None, size: None },
        })
        .collect();
    *PAINTINGS.write().unwrap_or_else(|p| p.into_inner()) = list;
}

fn gray(mut c: Component) -> Component {
    c.style.color = Some(0xaa_aaaa);
    c
}

/// Painting (`painting/variant`): title, author, dimensions; without a variant, "random" in creative.
pub fn painting(item: &str, variant: Option<&[u8]>, data: &rc_data::VersionData, out: &mut Vec<Component>) {
    if item != "minecraft:painting" {
        return;
    }
    let Some(b) = variant else {
        if creative() {
            out.push(gray(Component::translate("painting.random", vec![])));
        }
        return;
    };
    // Registry reference: index + 1 (0 = inline value, not handled).
    let Ok(id) = read_varint(&mut Reader::new(b)) else { return };
    let Some(index) = usize::try_from(id - 1).ok() else { return };
    let g = PAINTINGS.read().unwrap_or_else(|p| p.into_inner());
    let Some(p) = g.get(index) else { return };
    let key = |suffix: &str| {
        let (ns, path) = p.id.split_once(':').unwrap_or(("minecraft", &p.id));
        format!("painting.{ns}.{path}.{suffix}")
    };
    let colored = |mut c: Component, color: u32| {
        c.style.color = Some(color);
        c
    };
    out.push(p.title.clone().unwrap_or_else(|| colored(Component::translate(key("title"), vec![]), 0xff_ff55)));
    out.push(p.author.clone().unwrap_or_else(|| colored(Component::translate(key("author"), vec![]), 0xaa_aaaa)));
    let (w, h) = p
        .size
        .or_else(|| {
            let e = data.creative.paintings.iter().find(|e| e.0 == p.id)?;
            Some((i32::from(e.1), i32::from(e.2)))
        })
        .unwrap_or((1, 1));
    out.push(Component::translate("painting.dimensions", vec![Arg::Text(w.to_string()), Arg::Text(h.to_string())]));
}

/// Tropical fish patterns in the order of their network id (size | index << 8).
const PATTERNS: [(i32, &str); 12] = [
    (0, "kob"),
    (256, "sunstreak"),
    (512, "snooper"),
    (768, "dasher"),
    (1024, "brinely"),
    (1280, "spotty"),
    (1, "flopper"),
    (257, "stripey"),
    (513, "glitter"),
    (769, "blockfish"),
    (1025, "betty"),
    (1281, "clayfish"),
];

/// Common variants (pattern, base color, pattern color); colors = dye index.
const COMMON: [(&str, u8, u8); 22] = [
    ("stripey", 1, 7),
    ("flopper", 7, 7),
    ("flopper", 7, 11),
    ("clayfish", 0, 7),
    ("sunstreak", 11, 7),
    ("kob", 1, 0),
    ("spotty", 6, 3),
    ("blockfish", 10, 4),
    ("clayfish", 0, 14),
    ("spotty", 0, 4),
    ("glitter", 0, 7),
    ("clayfish", 0, 1),
    ("dasher", 9, 6),
    ("brinely", 5, 3),
    ("betty", 14, 0),
    ("snooper", 7, 14),
    ("blockfish", 14, 0),
    ("flopper", 0, 4),
    ("kob", 14, 0),
    ("sunstreak", 7, 0),
    ("dasher", 9, 4),
    ("flopper", 4, 4),
];

const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

fn italic_gray(mut c: Component) -> Component {
    c.style.italic = Some(true);
    gray(c)
}

/// Tropical fish (components `tropical_fish/pattern`, `base_color`, `pattern_color`).
pub fn tropical_fish(pattern: &[u8], base: Option<&[u8]>, color: Option<&[u8]>, out: &mut Vec<Component>) {
    let num = |b: &[u8]| read_varint(&mut Reader::new(b)).ok();
    let Some(id) = num(pattern) else { return };
    let Some(&(_, name)) = PATTERNS.iter().find(|p| p.0 == id).or(PATTERNS.first()) else { return };
    // Game defaults: kob pattern, white, white.
    let dye = |b: Option<&[u8]>| b.and_then(num).map_or(0, |v| v.clamp(0, 15) as u8);
    let (base, color) = (dye(base), dye(color));
    if let Some(i) = COMMON.iter().position(|c| *c == (name, base, color)) {
        out.push(italic_gray(Component::translate(format!("entity.minecraft.tropical_fish.predefined.{i}"), vec![])));
        return;
    }
    out.push(italic_gray(Component::translate(format!("entity.minecraft.tropical_fish.type.{name}"), vec![])));
    let mut colors = Component::translate(format!("color.minecraft.{}", DYES[base as usize]), vec![]);
    if base != color {
        colors.extra.push(Component::text(", "));
        colors.extra.push(Component::translate(format!("color.minecraft.{}", DYES[color as usize]), vec![]));
    }
    out.push(italic_gray(colors));
}

/// Sign text (front or back face): gray title then the four indented lines.
pub fn sign_text(front: bool, b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let mut lines = Vec::new();
    for _ in 0..4 {
        let Ok(Some(v)) = nbt::read(&mut r) else { return };
        let Some(c) = rc_text::from_nbt(&v) else { return };
        lines.push(c);
    }
    let Ok(has_filtered) = r.u8() else { return };
    let mut filtered_empty = true;
    if has_filtered != 0 {
        for _ in 0..4 {
            let Ok(Some(v)) = nbt::read(&mut r) else { return };
            filtered_empty &= rc_text::from_nbt(&v).is_some_and(|c| is_empty(&c));
        }
    }
    let (Ok(color), Ok(glow)) = (read_varint(&mut r), r.u8()) else { return };
    // Empty text by default: no line.
    if color == 0 && glow == 0 && filtered_empty && lines.iter().all(is_empty) {
        return;
    }
    out.push(gray(Component::translate(if front { "sign.front_text" } else { "sign.back_text" }, vec![])));
    for l in lines {
        let mut c = Component::text("  ");
        c.extra.push(l);
        out.push(c);
    }
}

fn is_empty(c: &Component) -> bool {
    matches!(&c.content, rc_text::Content::Text(t) if t.is_empty()) && c.extra.is_empty()
}

/// Suspicious stew: effects listed in creative only. Returns (effect, amplifier, duration).
pub fn stew_effects(b: &[u8]) -> Vec<(usize, i32, i32)> {
    if !creative() {
        return Vec::new();
    }
    let mut r = Reader::new(b);
    let Ok(n) = read_varint(&mut r) else { return Vec::new() };
    let mut out = Vec::new();
    for _ in 0..n.clamp(0, 64) {
        let (Ok(e), Ok(d)) = (read_varint(&mut r), read_varint(&mut r)) else { break };
        out.push((e as usize, 0, d));
    }
    out
}

/// Spawner (`spawner`, `trial_spawner`): mob name from `SpawnData`, otherwise explanation.
pub fn spawner(item: &str, data: Option<&[u8]>, out: &mut Vec<Component>) {
    if item != "minecraft:spawner" && item != "minecraft:trial_spawner" {
        return;
    }
    // Typed data: block-entity type id, then NBT.
    let entity = data.and_then(|b| {
        let mut r = Reader::new(b);
        read_varint(&mut r).ok()?;
        let tag = nbt::read(&mut r).ok()??;
        let id = tag.get("SpawnData")?.get("entity")?.get("id")?.as_str()?.to_owned();
        Some(id)
    });
    if let Some(id) = entity {
        let path = id.strip_prefix("minecraft:").unwrap_or(&id).replace(':', ".");
        out.push(gray(Component::translate(format!("entity.minecraft.{path}"), vec![])));
    } else {
        out.push(Component::text(""));
        out.push(gray(Component::translate("block.minecraft.spawner.desc1", vec![])));
        let mut c = Component::text(" ");
        let mut d = Component::translate("block.minecraft.spawner.desc2", vec![]);
        d.style.color = Some(0x55_55ff);
        c.extra.push(d);
        out.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: &Component) -> String {
        match &c.content {
            rc_text::Content::Translate { key, .. } => key.clone(),
            rc_text::Content::Text(t) => t.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn tropical_fish_common_and_custom() {
        let mut out = Vec::new();
        // Anemone: stripey pattern (257), orange (1), gray (7).
        tropical_fish(&[0x81, 0x02], Some(&[1]), Some(&[7]), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(key(&out[0]), "entity.minecraft.tropical_fish.predefined.0");
        // Free variant: pattern name, then the two tints.
        let mut out = Vec::new();
        tropical_fish(&[0], Some(&[3]), Some(&[5]), &mut out);
        assert_eq!(key(&out[0]), "entity.minecraft.tropical_fish.type.kob");
        assert_eq!(key(&out[1]), "color.minecraft.light_blue");
        assert_eq!(out[1].extra.len(), 2);
        // Two equal tints: just one.
        let mut out = Vec::new();
        tropical_fish(&[0], Some(&[3]), Some(&[3]), &mut out);
        assert!(out[1].extra.is_empty());
    }

    #[test]
    fn spawner_lines() {
        let mut out = Vec::new();
        spawner("minecraft:spawner", None, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(key(&out[1]), "block.minecraft.spawner.desc1");
        let mut out = Vec::new();
        spawner("minecraft:stone", None, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn stew_only_in_creative() {
        // One effect: id 5, duration 160 (varint 0xa0 0x01).
        let b = [1u8, 5, 0xa0, 0x01];
        set_creative(false);
        assert!(stew_effects(&b).is_empty());
        set_creative(true);
        assert_eq!(stew_effects(&b), vec![(5, 0, 160)]);
        set_creative(false);
    }

    #[test]
    fn empty_sign_text_is_silent() {
        // Four empty strings (text NBT: type 8, length 0), no filter, black, no glow.
        let mut b = Vec::new();
        for _ in 0..4 {
            b.extend_from_slice(&[8, 0, 0]);
        }
        b.extend_from_slice(&[0, 0, 0]);
        let mut out = Vec::new();
        sign_text(true, &b, &mut out);
        assert!(out.is_empty());
    }
}
