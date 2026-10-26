//! Tooltip sections specific to certain components (container, banner patterns, armor
//! trim, written book, fireworks, crossbow, decorated pot, beehive, disc, horn, smithing
//! templates): lines in the order and with the colors of the official game. The synchronized
//! registries (trims, instruments, songs) are remembered when received.

use std::sync::RwLock;

use rc_proto::Reader;
use rc_proto::varint::read_varint;
use rc_proto_v777::item::ItemStack;
use rc_text::{Arg, Component};

use crate::stack::StackRef;

const GRAY: u32 = 0xaa_aaaa;

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

/// Firework colors of the dyes (same order as `DYES`).
const FIREWORK_COLORS: [i32; 16] = [
    15_790_320, 15_435_844, 12_801_229, 6_719_955, 14_602_026, 4_312_372, 14_188_952, 4_408_131, 11_250_603, 2_651_799,
    8_073_150, 2_437_522, 5_320_730, 3_887_386, 11_743_532, 1_973_019,
];

const SHAPES: [&str; 5] = ["small_ball", "large_ball", "star", "creeper", "burst"];

/// Tints of the trim material descriptions (base game).
const TRIM_COLORS: [(&str, u32); 11] = [
    ("quartz", 0xe3_d4c4),
    ("iron", 0xec_ecec),
    ("netherite", 0x62_5859),
    ("redstone", 0x97_1607),
    ("copper", 0xb4_684d),
    ("gold", 0xde_b12d),
    ("emerald", 0x11_a036),
    ("diamond", 0x6e_ecd2),
    ("lapis", 0x41_6e97),
    ("amethyst", 0x9a_5cc6),
    ("resin", 0xfc_7812),
];

/// Entry of a remembered registry: id and description (content text if it is sent).
type Entry = (String, Option<Component>);

static REGS: RwLock<Vec<(String, Vec<Entry>)>> = RwLock::new(Vec::new());

/// Registries whose descriptions serve the tooltips.
pub fn wants(name: &str) -> bool {
    matches!(
        name,
        "minecraft:trim_material" | "minecraft:trim_pattern" | "minecraft:instrument" | "minecraft:jukebox_song"
    )
}

/// Remembers a synchronized registry (ids in network order, possible description).
pub fn set_registry(name: &str, entries: &rc_client::RegistryEntries) {
    crate::tooltips_more::set_registry(name, entries);
    if !wants(name) {
        return;
    }
    let list: Vec<Entry> = entries
        .iter()
        .map(|(n, nbt)| (n.clone(), nbt.as_ref().and_then(|v| v.get("description")).and_then(rc_text::from_nbt)))
        .collect();
    let mut g = REGS.write().unwrap_or_else(|p| p.into_inner());
    g.retain(|(n, _)| n != name);
    g.push((name.to_owned(), list));
}

fn registry_entry(reg: &str, index: usize) -> Option<Entry> {
    let g = REGS.read().unwrap_or_else(|p| p.into_inner());
    g.iter().find(|(n, _)| n == reg)?.1.get(index).cloned()
}

fn colored(mut c: Component, color: u32) -> Component {
    c.style.color = Some(color);
    c
}

fn gray(c: Component) -> Component {
    colored(c, GRAY)
}

fn key_of(id: &str) -> String {
    id.replace(':', ".")
}

/// Component preceded by a space ("indented lines" of the game).
fn spaced(c: Component) -> Component {
    let mut line = Component::text(" ");
    line.extra.push(c);
    line
}

/// Hover name of the stack (without rarity tint).
pub fn hover_name(v: &StackRef<'_>) -> Component {
    let mut c = v.display_name();
    if c.extra.len() == 1 { c.extra.remove(0) } else { c }
}

/// Nested stack seen as `StackRef` (same tables as the parent stack).
pub fn nested<'a>(v: &StackRef<'a>, st: &'a ItemStack) -> StackRef<'a> {
    let info = usize::try_from(st.item).ok().and_then(|i| v.data.items.get(i));
    StackRef { stack: st, info, ids: v.ids, data: v.data, hold: crate::stack::Hold::NONE }
}

/// Stack references ("templates") of a container: empty slots ignored.
fn container_items(v: &StackRef<'_>, b: &[u8]) -> Vec<ItemStack> {
    let mut r = Reader::new(b);
    let Ok(n) = rc_proto::read_len(&mut r, 256) else { return Vec::new() };
    let mut out = Vec::new();
    for _ in 0..n {
        match r.u8() {
            Ok(1) => match v.ids.codec.read_template(&mut r) {
                Ok(s) => out.push(s),
                Err(_) => break,
            },
            Ok(_) => {}
            Err(_) => break,
        }
    }
    out
}

/// Stacks of a list of templates (crossbow arrows, bundle contents).
fn template_list(v: &StackRef<'_>, b: &[u8]) -> Vec<ItemStack> {
    let mut r = Reader::new(b);
    let Ok(n) = read_varint(&mut r) else { return Vec::new() };
    let mut out = Vec::new();
    for _ in 0..n.clamp(0, 256) {
        match v.ids.codec.read_template(&mut r) {
            Ok(s) => out.push(s),
            Err(_) => break,
        }
    }
    out
}

/// Contents of a container: at most five lines (game behavior), then "and N more…".
pub fn container(v: &StackRef<'_>, b: &[u8], out: &mut Vec<Component>) {
    let items = container_items(v, b);
    let mut lines = 0;
    for st in &items {
        if lines <= 4 {
            lines += 1;
            let nv = nested(v, st);
            out.push(Component::translate(
                "item.container.item_count",
                vec![Arg::Text(st.count.to_string()), Arg::Component(hover_name(&nv))],
            ));
        }
    }
    if items.len() > lines {
        let mut c =
            Component::translate("item.container.more_items", vec![Arg::Text((items.len() - lines).to_string())]);
        c.style.italic = Some(true);
        out.push(c);
    }
}

/// Banner patterns: at most six lines, "Pattern Color" in gray.
pub fn banner_patterns(b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let Ok(n) = read_varint(&mut r) else { return };
    for i in 0..n.clamp(0, 256) {
        let Ok(id) = read_varint(&mut r) else { return };
        let key = if id == 0 {
            let (Ok(_), Ok(k)) = (rc_proto::read_str(&mut r, 32767), rc_proto::read_str(&mut r, 32767)) else { return };
            k.to_owned()
        } else {
            let list = crate::screens_work::PATTERNS.read().unwrap_or_else(|p| p.into_inner());
            match list.get((id - 1) as usize) {
                Some(p) => p.2.clone(),
                None => return,
            }
        };
        let Ok(color) = read_varint(&mut r) else { return };
        if i < 6 {
            let dye = DYES.get(color as usize).copied().unwrap_or("white");
            out.push(gray(Component::translate(format!("{key}.{dye}"), vec![])));
        }
    }
}

/// Tint of a material: that of its description, otherwise that of the base game materials.
fn material_color(path: &str, desc: Option<&Component>) -> Option<u32> {
    desc.and_then(|d| d.style.color).or_else(|| TRIM_COLORS.iter().find(|(n, _)| *n == path).map(|c| c.1))
}

/// Reads a reference (registry id + 1, or direct value): (id, description).
fn holder(r: &mut Reader<'_>, reg: &str, direct: impl FnOnce(&mut Reader<'_>) -> Option<Entry>) -> Option<Entry> {
    let v = read_varint(r).ok()?;
    if v == 0 { direct(r) } else { registry_entry(reg, (v - 1) as usize) }
}

fn description(r: &mut Reader<'_>) -> Option<Option<Component>> {
    Some(rc_proto::nbt::read(r).ok()?.and_then(|n| rc_text::from_nbt(&n)))
}

/// Armor trim: "Upgrade:", pattern then material (material tint).
pub fn trim(b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let Some((mat, mat_desc)) = holder(&mut r, "minecraft:trim_material", |r| {
        let id = rc_proto::read_str(r, 32767).ok()?.to_owned();
        Some((id, description(r)?))
    }) else {
        return;
    };
    let Some((pat, pat_desc)) = holder(&mut r, "minecraft:trim_pattern", |r| {
        let id = rc_proto::read_str(r, 32767).ok()?.to_owned();
        let d = description(r)?;
        r.u8().ok()?;
        Some((id, d))
    }) else {
        return;
    };
    let mat_path = mat.strip_prefix("minecraft:").unwrap_or(&mat);
    let mat_desc = mat_desc.unwrap_or_else(|| Component::translate(format!("trim_material.{}", key_of(&mat)), vec![]));
    let color = material_color(mat_path, Some(&mat_desc));
    let mut pat_desc =
        pat_desc.unwrap_or_else(|| Component::translate(format!("trim_pattern.{}", key_of(&pat)), vec![]));
    if let Some(c) = color {
        pat_desc.style.color = Some(c);
    }
    let mut mat_line = mat_desc;
    if mat_line.style.color.is_none() {
        mat_line.style.color = color;
    }
    out.push(gray(Component::translate("item.minecraft.smithing_template.upgrade", vec![])));
    out.push(spaced(pat_desc));
    out.push(spaced(mat_line));
}

/// Author and generation of a written book.
pub fn written_book(b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    if rc_proto::read_str(&mut r, 32).is_err() {
        return;
    }
    match r.u8() {
        Ok(1) => {
            if rc_proto::read_str(&mut r, 32).is_err() {
                return;
            }
        }
        Ok(_) => {}
        Err(_) => return,
    }
    let Ok(author) = rc_proto::read_str(&mut r, 32767) else { return };
    let Ok(gen_) = read_varint(&mut r) else { return };
    if !author.trim().is_empty() {
        out.push(gray(Component::translate("book.byAuthor", vec![Arg::Text(author.to_owned())])));
    }
    out.push(gray(Component::translate(format!("book.generation.{gen_}"), vec![])));
}

/// Firework explosion: shape, colors, fade, trail, twinkle.
#[derive(Clone, PartialEq)]
struct Explosion {
    shape: usize,
    colors: Vec<i32>,
    fade: Vec<i32>,
    trail: bool,
    twinkle: bool,
}

fn explosion(r: &mut Reader<'_>) -> Option<Explosion> {
    let shape = read_varint(r).ok()?;
    let colors = |r: &mut Reader<'_>| -> Option<Vec<i32>> {
        let n = read_varint(r).ok()?.clamp(0, 256);
        (0..n).map(|_| Some(i32::from_be_bytes(r.take(4).ok()?.try_into().ok()?))).collect()
    };
    let c = colors(r)?;
    let f = colors(r)?;
    let trail = r.u8().ok()? != 0;
    let twinkle = r.u8().ok()? != 0;
    Some(Explosion {
        shape: usize::try_from(shape).ok().filter(|s| *s < SHAPES.len()).unwrap_or(0),
        colors: c,
        fade: f,
        trail,
        twinkle,
    })
}

fn shape_name(e: &Explosion) -> Component {
    Component::translate(format!("item.minecraft.firework_star.shape.{}", SHAPES[e.shape]), vec![])
}

fn colors_line(mut head: Component, colors: &[i32]) -> Component {
    for (i, c) in colors.iter().enumerate() {
        if i > 0 {
            head.extra.push(Component::text(", "));
        }
        head.extra.push(match FIREWORK_COLORS.iter().position(|x| x == c) {
            Some(d) => Component::translate(format!("item.minecraft.firework_star.{}", DYES[d]), vec![]),
            None => Component::translate("item.minecraft.firework_star.custom_color", vec![]),
        });
    }
    head
}

fn explosion_extra(e: &Explosion, indent: bool, out: &mut Vec<Component>) {
    let mut lines = Vec::new();
    if !e.colors.is_empty() {
        lines.push(gray(colors_line(Component::text(""), &e.colors)));
    }
    if !e.fade.is_empty() {
        let mut head = Component::translate("item.minecraft.firework_star.fade_to", vec![]);
        head.extra.push(Component::text(" "));
        lines.push(gray(colors_line(head, &e.fade)));
    }
    if e.trail {
        lines.push(gray(Component::translate("item.minecraft.firework_star.trail", vec![])));
    }
    if e.twinkle {
        lines.push(gray(Component::translate("item.minecraft.firework_star.flicker", vec![])));
    }
    for l in lines {
        if indent {
            let mut c = Component::text("  ");
            c.extra.push(l);
            out.push(c);
        } else {
            out.push(l);
        }
    }
}

/// Firework star: shape then details.
pub fn firework_star(b: &[u8], out: &mut Vec<Component>) {
    let Some(e) = explosion(&mut Reader::new(b)) else { return };
    out.push(gray(shape_name(&e)));
    explosion_extra(&e, false, out);
}

/// Rocket: flight duration, then each explosion (identical ones grouped).
pub fn fireworks(b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let Ok(flight) = read_varint(&mut r) else { return };
    let Ok(n) = rc_proto::read_len(&mut r, 256) else { return };
    let list: Vec<Explosion> = (0..n).map_while(|_| explosion(&mut r)).collect();
    if flight > 0 {
        out.push(gray(Component::translate(
            "item.minecraft.firework_rocket.flight_duration",
            vec![Arg::Text(flight.to_string())],
        )));
    }
    let mut i = 0;
    while i < list.len() {
        let mut count = 1;
        while i + count < list.len() && list[i + count] == list[i] {
            count += 1;
        }
        let shape = Arg::Component(shape_name(&list[i]));
        out.push(gray(if count == 1 {
            Component::translate("item.minecraft.firework_rocket.single_star", vec![shape])
        } else {
            Component::translate(
                "item.minecraft.firework_rocket.multiple_stars",
                vec![Arg::Text(count.to_string()), shape],
            )
        }));
        explosion_extra(&list[i], true, out);
        i += count;
    }
}

/// Loaded crossbow: grouped projectiles, then their offset details in gray.
pub fn charged_projectiles(
    v: &StackRef<'_>,
    b: &[u8],
    details: &dyn Fn(&StackRef<'_>, &mut Vec<Component>),
    out: &mut Vec<Component>,
) {
    let items = template_list(v, b);
    let mut i = 0;
    while i < items.len() {
        let mut count = 1;
        while i + count < items.len() && items[i + count] == items[i] {
            count += 1;
        }
        let nv = nested(v, &items[i]);
        if count == 1 {
            let mut shown = Component::translate("chat.square_brackets", vec![Arg::Component(hover_name(&nv))]);
            shown.style.color = nv.display_name().style.color;
            out.push(Component::translate("item.minecraft.crossbow.projectile.single", vec![Arg::Component(shown)]));
        } else {
            out.push(Component::translate(
                "item.minecraft.crossbow.projectile.multiple",
                vec![Arg::Text(count.to_string()), Arg::Component(hover_name(&nv))],
            ));
        }
        let mut sub = Vec::new();
        details(&nv, &mut sub);
        for l in sub {
            let mut c = Component::text("  ");
            c.extra.push(l);
            out.push(gray(c));
        }
        i += count;
    }
}

/// Decorated pot: an empty line then the name of each face present (front, left, right, back).
pub fn pot_decorations(v: &StackRef<'_>, b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let mut sides: Vec<Option<ItemStack>> = Vec::new();
    for _ in 0..4 {
        match r.u8() {
            Ok(1) => sides.push(v.ids.codec.read_template(&mut r).ok()),
            Ok(_) => sides.push(None),
            Err(_) => return,
        }
    }
    if sides.iter().all(Option::is_none) {
        return;
    }
    out.push(Component::text(""));
    // Network order: back, left, right, front.
    for k in [3, 1, 2, 0] {
        if let Some(st) = &sides[k] {
            out.push(gray(hover_name(&nested(v, st))));
        }
    }
}

/// Number of bees of a hive or nest.
pub fn bees(b: &[u8], out: &mut Vec<Component>) {
    let Ok(n) = read_varint(&mut Reader::new(b)) else { return };
    out.push(gray(Component::translate(
        "container.beehive.bees",
        vec![Arg::Text(n.to_string()), Arg::Text("3".into())],
    )));
}

/// Honey level of a hive (block state property of the item).
pub fn block_state(b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let Ok(n) = read_varint(&mut r) else { return };
    for _ in 0..n.clamp(0, 64) {
        let (Ok(k), Ok(val)) = (rc_proto::read_str(&mut r, 32767), rc_proto::read_str(&mut r, 32767)) else { return };
        if k == "honey_level"
            && let Ok(level) = val.parse::<i32>()
        {
            out.push(gray(Component::translate(
                "container.beehive.honey",
                vec![Arg::Text(level.to_string()), Arg::Text("5".into())],
            )));
        }
    }
}

/// Disc, horn: description of the registry entry, in gray.
fn registry_description(prefix: &str, entry: Entry) -> Component {
    let (id, desc) = entry;
    let c = desc.unwrap_or_else(|| Component::translate(format!("{prefix}.{}", key_of(&id)), vec![]));
    let mut c = c;
    if c.style.color.is_none() {
        c.style.color = Some(GRAY);
    }
    c
}

/// Song of an item playable in a jukebox.
pub fn jukebox_playable(b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let entry = holder(&mut r, "minecraft:jukebox_song", |r| {
        // Sound (reference or direct value), description, duration, signal.
        if read_varint(r).ok()? == 0 {
            rc_proto::read_str(r, 32767).ok()?;
            if r.u8().ok()? == 1 {
                r.take(4).ok()?;
            }
        }
        Some((String::new(), description(r)?))
    });
    if let Some(e) = entry {
        out.push(registry_description("jukebox_song", e));
    }
}

/// Goat horn: description of the instrument, in gray.
pub fn instrument(b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let entry = holder(&mut r, "minecraft:instrument", |r| {
        if read_varint(r).ok()? == 0 {
            rc_proto::read_str(r, 32767).ok()?;
            if r.u8().ok()? == 1 {
                r.take(4).ok()?;
            }
        }
        r.take(8).ok()?;
        read_varint(r).ok()?;
        Some((String::new(), description(r)?))
    });
    if let Some(e) = entry {
        out.push(registry_description("instrument", e));
    }
}

/// Block registry tags (name without namespace → ids), received from the server.
static BLOCK_TAGS: RwLock<Vec<(String, Vec<i32>)>> = RwLock::new(Vec::new());

/// Remembers the block tags ("Can break" / "Can be placed on" predicates).
pub fn set_block_tags(tags: &rc_proto_v777::play::Tags) {
    let Some((_, list)) = tags.iter().find(|(r, _)| r == "minecraft:block") else { return };
    let v = list.iter().map(|(n, ids)| (n.strip_prefix("minecraft:").unwrap_or(n).to_owned(), ids.clone())).collect();
    *BLOCK_TAGS.write().unwrap_or_else(|p| p.into_inner()) = v;
}

/// Blocks (registry ids) of a set of references: list or tag.
fn holder_blocks(r: &mut Reader<'_>) -> Option<Vec<i32>> {
    let n = read_varint(r).ok()?;
    if n == 0 {
        let tag = rc_proto::read_str(r, 32767).ok()?;
        let tag = tag.strip_prefix("minecraft:").unwrap_or(tag);
        let g = BLOCK_TAGS.read().unwrap_or_else(|p| p.into_inner());
        return Some(g.iter().find(|(t, _)| t == tag).map(|t| t.1.clone()).unwrap_or_default());
    }
    (1..n).map(|_| read_varint(r).ok()).collect()
}

/// "Can break" / "Can be placed on": header then distinct block names (dark gray).
/// Only the predicates carrying blocks (without properties or data) are read; a richer predicate
/// stops the reading.
pub fn block_predicates(view: &StackRef<'_>, header: &str, b: &[u8], out: &mut Vec<Component>) {
    let mut r = Reader::new(b);
    let Ok(n) = read_varint(&mut r) else { return };
    let mut blocks: Vec<i32> = Vec::new();
    let mut unknown = false;
    for _ in 0..n.clamp(0, 64) {
        let Ok(has) = r.u8() else { break };
        if has == 1 {
            let Some(list) = holder_blocks(&mut r) else { break };
            for id in list {
                if !blocks.contains(&id) {
                    blocks.push(id);
                }
            }
        } else {
            unknown = true;
        }
        // Properties, block data, components and partial predicates: only "absent".
        let rest: Option<()> = (|| {
            (r.u8().ok()? == 0).then_some(())?;
            (r.u8().ok()? == 0).then_some(())?;
            (read_varint(&mut r).ok()? == 0).then_some(())?;
            (read_varint(&mut r).ok()? == 0).then_some(())
        })();
        if rest.is_none() {
            break;
        }
    }
    out.push(Component::text(""));
    out.push(gray(Component::translate(header, vec![])));
    if unknown {
        out.push(gray(Component::translate("item.canUse.unknown", vec![])));
        return;
    }
    let names = view.data.registry("minecraft:block");
    for id in blocks {
        let Some(name) = usize::try_from(id).ok().and_then(|i| names.and_then(|r| r.entries.get(i))) else { continue };
        out.push(colored(Component::translate(format!("block.{}", key_of(name)), vec![]), 0x55_5555));
    }
}

/// Music disc: description of the song of the same name (default component of the item).
pub fn disc_song(name: &str, out: &mut Vec<Component>) {
    let id = format!("minecraft:{name}");
    let c = {
        let g = REGS.read().unwrap_or_else(|p| p.into_inner());
        g.iter().find(|(n, _)| n == "minecraft:jukebox_song").and_then(|(_, l)| l.iter().find(|e| e.0 == id).cloned())
    };
    let (id, desc) = c.unwrap_or((id, None));
    out.push(registry_description("jukebox_song", (id, desc)));
}

/// Lines specific to the item (smithing templates, disc fragment), before the components.
pub fn item_specific(item: &str, out: &mut Vec<Component>) {
    let path = item.strip_prefix("minecraft:").unwrap_or(item);
    let blue = |k: &str| colored(Component::translate(k, vec![]), 0x55_55ff);
    let title = |k: &str| gray(Component::translate(k, vec![]));
    let template = |kind: &str, out: &mut Vec<Component>| {
        out.push(title("item.minecraft.smithing_template"));
        out.push(Component::text(""));
        out.push(title("item.minecraft.smithing_template.applies_to"));
        out.push(spaced(blue(&format!("item.minecraft.smithing_template.{kind}.applies_to"))));
        out.push(title("item.minecraft.smithing_template.ingredients"));
        out.push(spaced(blue(&format!("item.minecraft.smithing_template.{kind}.ingredients"))));
    };
    if path == "netherite_upgrade_smithing_template" {
        template("netherite_upgrade", out);
    } else if path.ends_with("_armor_trim_smithing_template") {
        template("armor_trim", out);
    } else if path.starts_with("disc_fragment_") {
        out.push(gray(Component::translate(format!("item.minecraft.{path}.desc"), vec![])));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firework_colors_match_dyes() {
        assert_eq!(FIREWORK_COLORS.len(), DYES.len());
        assert_eq!(FIREWORK_COLORS[14], 11_743_532);
    }

    #[test]
    fn book_lines() {
        // title "T", no filtered title, author "Bob", generation 2.
        let mut b = vec![1, b'T', 0, 3, b'B', b'o', b'b', 2];
        b.extend([0, 0]);
        let mut out = Vec::new();
        written_book(&b, &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn block_sets() {
        // List of two blocks (size + 1, then ids).
        assert_eq!(holder_blocks(&mut Reader::new(&[3, 5, 6])), Some(vec![5, 6]));
        // Unknown tag: empty set.
        let mut tag = vec![0u8, 4];
        tag.extend_from_slice(b"logs");
        assert_eq!(holder_blocks(&mut Reader::new(&tag)), Some(Vec::new()));
    }

    #[test]
    fn fireworks_group_equal_explosions() {
        // Duration 1, two identical explosions (shape 2, one red color, no fade, no trail).
        let mut b = vec![1u8, 2];
        for _ in 0..2 {
            b.push(2);
            b.extend([1, 0x00, 0xb3, 0x31, 0x2c]);
            b.extend([0, 0, 0]);
        }
        let mut out = Vec::new();
        fireworks(&b, &mut out);
        // Flight duration, then "2 × shape" and the color line.
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn bees_line() {
        let mut out = Vec::new();
        bees(&[2], &mut out);
        assert_eq!(out.len(), 1);
    }
}
