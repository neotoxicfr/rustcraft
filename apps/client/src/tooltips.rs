//! Item tooltip lines, in the game's order: stack data (network patch),
//! then item default values for the attributes. Components hidden by
//! `tooltip_display` are omitted; the order of the enchantments follows the `tooltip_order` tag
//! received from the server (then the stack's order), curses are in red.

use hashbrown::HashSet;
use std::sync::RwLock;

use rc_proto::Reader;
use rc_proto::varint::read_varint;
use rc_text::{Arg, Component};

use crate::stack::StackRef;
use rc_assets::item::StackView as _;

/// Enchantment tags received: display order and curses (registry indices).
#[derive(Default)]
struct Known {
    order: Vec<i32>,
    curses: Vec<i32>,
}

static KNOWN: RwLock<Known> = RwLock::new(Known { order: Vec::new(), curses: Vec::new() });

/// Tags of the `minecraft:enchantment` registry (`tooltip_order`, `curse`).
pub fn set_tags(tags: &rc_proto_v777::play::Tags) {
    let Some((_, list)) = tags.iter().find(|(r, _)| r == "minecraft:enchantment") else { return };
    let find = |name: &str| {
        list.iter().find(|(n, _)| n.strip_prefix("minecraft:").unwrap_or(n) == name).map(|(_, v)| v.clone())
    };
    let mut g = KNOWN.write().unwrap_or_else(|p| p.into_inner());
    g.order = find("tooltip_order").unwrap_or_default();
    g.curses = find("curse").unwrap_or_default();
}

fn colored(mut c: Component, color: u32) -> Component {
    c.style.color = Some(color);
    c
}

/// Number with at most two decimals, trailing zeros removed (the game's `#.##` format).
fn num(x: f64) -> String {
    let r = (x * 100.0).round_ties_even() / 100.0;
    let s = format!("{r:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// Duration of an effect: "mm:ss" ("hh:mm:ss" beyond an hour).
fn duration(ticks: i32) -> String {
    let secs = ticks / 20;
    let (m, s) = (secs / 60, secs % 60);
    if m >= 60 { format!("{:02}:{:02}:{:02}", m / 60, m % 60, s) } else { format!("{m:02}:{s:02}") }
}

fn arg(c: Component) -> Arg {
    Arg::Component(c)
}

/// Components hidden by `tooltip_display`: (whole tooltip hidden, types hidden).
fn display(view: &StackRef<'_>) -> (bool, Vec<u16>) {
    let Some(b) = view.patch("minecraft:tooltip_display") else { return (false, Vec::new()) };
    let mut r = Reader::new(b);
    let hide = r.u8().is_ok_and(|v| v != 0);
    let n = read_varint(&mut r).unwrap_or(0).max(0);
    (hide, (0..n).filter_map(|_| read_varint(&mut r).ok().map(|v| v as u16)).collect())
}

/// Check log (`RUSTCRAFT_TIPLOG`): tooltip text of each stack seen, once.
pub fn log_enabled() -> bool {
    rc_data::env_flag!("RUSTCRAFT_TIPLOG")
}

/// Writes the text of a stack's tooltip lines, once per distinct stack.
pub fn log_once(key: &str, lines: &[Vec<rc_text::StyledChar>]) {
    static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    if seen.iter().any(|s| s == key) {
        return;
    }
    seen.push(key.to_owned());
    eprintln!("tooltip {key}");
    for l in lines {
        eprintln!("  | {}", l.iter().map(|c| c.c).collect::<String>());
    }
}

/// The whole tooltip is hidden (`tooltip_display.hide_tooltip`).
pub fn hidden(view: &StackRef<'_>) -> bool {
    display(view).0
}

/// Tooltip context, set on every world tick: bit 0 peaceful difficulty, bit 1
/// operator player (permission 2 at least).
static CONTEXT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_context(peaceful: bool, operator: bool) {
    CONTEXT.store(u8::from(peaceful) | u8::from(operator) << 1, std::sync::atomic::Ordering::Relaxed);
}

fn context() -> (bool, bool) {
    let c = CONTEXT.load(std::sync::atomic::Ordering::Relaxed);
    (c & 1 != 0, c & 2 != 0)
}

/// Name of the type of typed data (`entity_data`, `block_entity_data`): rank in `registry`.
fn typed_data_type<'a>(view: &StackRef<'a>, component: &str, registry: &str) -> Option<&'a str> {
    let b = view.patch(component)?;
    let i = read_varint(&mut Reader::new(b)).ok()?;
    view.data.registry(registry)?.entries.get(usize::try_from(i).ok()?).map(String::as_str)
}

/// Entity and block entity types for which only the data set by an operator is kept.
const OP_ONLY_ENTITIES: [&str; 3] = ["minecraft:falling_block", "minecraft:command_block_minecart", "minecraft:spawner_minecart"];
const OP_ONLY_BLOCK_ENTITIES: [&str; 6] = [
    "minecraft:command_block",
    "minecraft:lectern",
    "minecraft:sign",
    "minecraft:hanging_sign",
    "minecraft:mob_spawner",
    "minecraft:trial_spawner",
];

/// Operator warning: spawn egg or block item carrying data reserved
/// for operators (three red lines, the first in bold); empty otherwise.
pub fn op_warning(view: &StackRef<'_>, operator: bool, out: &mut Vec<Component>) {
    if !operator {
        return;
    }
    let item = view.item_id();
    let warn = if item.ends_with("_spawn_egg") {
        typed_data_type(view, "minecraft:entity_data", "minecraft:entity_type").is_some_and(|t| OP_ONLY_ENTITIES.contains(&t))
    } else {
        typed_data_type(view, "minecraft:block_entity_data", "minecraft:block_entity_type")
            .is_some_and(|t| OP_ONLY_BLOCK_ENTITIES.contains(&t))
            && {
                let block = rc_data::rules::placed_block(item);
                view.data.blocks.iter().any(|b| b.name == block)
            }
    };
    if warn {
        for i in 1..=3 {
            let mut c = colored(Component::translate(format!("item.op_warning.line{i}"), vec![]), 0xff_5555);
            if i == 1 {
                c.style.bold = Some(true);
            }
            out.push(c);
        }
    }
}

/// Lines of a hidden tooltip (outside creative): only the operator warning.
pub fn hidden_lines(view: &StackRef<'_>) -> Vec<Component> {
    let mut out = Vec::new();
    op_warning(view, context().1, &mut out);
    out
}

/// Spawn egg (or stack with entity data) of a type forbidden in peaceful, in peaceful:
/// red line. Type: that of the stack's data, otherwise that of the egg.
fn peaceful_line(view: &StackRef<'_>, peaceful: bool, shows: bool, out: &mut Vec<Component>) {
    if !peaceful || !shows {
        return;
    }
    let ty = match view.patch("minecraft:entity_data") {
        Some(_) => typed_data_type(view, "minecraft:entity_data", "minecraft:entity_type").map(str::to_owned),
        None => {
            let id = view.ids.of("minecraft:entity_data");
            let default = id.is_some_and(|i| view.info.is_some_and(|f| f.components.contains(&i)) && !view.stack.removed.contains(&i));
            view.item_id().strip_suffix("_spawn_egg").filter(|_| default).map(str::to_owned)
        }
    };
    if ty.is_some_and(|t| !rc_data::rules::allowed_in_peaceful(&t)) {
        out.push(colored(Component::translate("item.spawn_egg.peaceful", vec![]), 0xff_5555));
    }
}

/// Pairs (id, level) of an enchantments component.
fn pairs(b: &[u8]) -> Vec<(i32, i32)> {
    let mut r = Reader::new(b);
    let n = read_varint(&mut r).unwrap_or(0).max(0);
    (0..n).map_while(|_| Some((read_varint(&mut r).ok()?, read_varint(&mut r).ok()?))).collect()
}

/// Full name of an enchantment: description, red for a curse (`curse` tag,
/// otherwise `_curse` suffix), otherwise grey; level in Roman numerals except a single level.
pub(crate) fn enchantment_line(id: i32, level: i32, names: &[String], single: &[&str]) -> Option<Component> {
    let e = names.get(usize::try_from(id).ok()?)?;
    let path = e.strip_prefix("minecraft:").unwrap_or(e);
    let curse = {
        let g = KNOWN.read().unwrap_or_else(|p| p.into_inner());
        if g.curses.is_empty() { path.ends_with("_curse") } else { g.curses.contains(&id) }
    };
    let mut line = Component::text("");
    line.style.color = Some(if curse { 0xff_5555 } else { 0xaa_aaaa });
    line.extra.push(Component::translate(format!("enchantment.{}", e.replace(':', ".")), vec![]));
    if !(level == 1 && single.contains(&path)) {
        line.extra.push(Component::text(" "));
        line.extra.push(Component::translate(format!("enchantment.level.{level}"), vec![]));
    }
    Some(line)
}

/// Lines of an enchantments component, tag order first.
fn enchantments(b: &[u8], names: &[String], single: &[&str], out: &mut Vec<Component>) {
    let all = pairs(b);
    let g = KNOWN.read().unwrap_or_else(|p| p.into_inner());
    let mut seen = Vec::new();
    for id in &g.order {
        if let Some(&(i, l)) = all.iter().find(|p| p.0 == *id).filter(|p| p.1 > 0) {
            out.extend(enchantment_line(i, l, names, single));
            seen.push(i);
        }
    }
    for &(i, l) in &all {
        if !seen.contains(&i) && !g.order.contains(&i) {
            out.extend(enchantment_line(i, l, names, single));
        }
    }
}

/// Effects of a potion: (effect id, amplifier, duration in ticks).
fn potion_effects(view: &StackRef<'_>, b: &[u8]) -> Option<Vec<(usize, i32, i32)>> {
    let mut r = Reader::new(b);
    let potion = if r.u8().ok()? == 1 { Some(read_varint(&mut r).ok()? as usize) } else { None };
    if r.u8().ok()? == 1 {
        r.take(4).ok()?;
    }
    let mut out: Vec<(usize, i32, i32)> = potion
        .and_then(|p| view.data.potions.get(p))
        .into_iter()
        .flatten()
        .map(|e| (usize::from(e.effect), i32::from(e.amplifier), e.duration))
        .collect();
    let n = read_varint(&mut r).ok()?;
    for _ in 0..n {
        let effect = read_varint(&mut r).ok()? as usize;
        let amp = read_varint(&mut r).ok()?;
        let dur = read_varint(&mut r).ok()?;
        r.take(3).ok()?;
        if r.u8().ok()? == 1 {
            // Hidden effect: read and ignored (rare nesting).
            let mut depth = 0;
            loop {
                read_varint(&mut r).ok()?;
                read_varint(&mut r).ok()?;
                r.take(3).ok()?;
                depth += 1;
                if r.u8().ok()? != 1 || depth > 16 {
                    break;
                }
            }
        }
        out.push((effect, amp, dur));
    }
    Some(out)
}

/// A potion effect line: name, strength, duration, tint of the category.
fn effect_line(view: &StackRef<'_>, (effect, amp, dur): (usize, i32, i32), scale: f32) -> Option<Component> {
    let name = view.data.registry("minecraft:mob_effect")?.entries.get(effect)?;
    let mut line = Component::translate(format!("effect.{}", name.replace(':', ".")), vec![]);
    if amp > 0 {
        line = Component::translate(
            "potion.withAmplifier",
            vec![arg(line), arg(Component::translate(format!("potion.potency.{amp}"), vec![]))],
        );
    }
    if dur == -1 || dur > 20 {
        let text = if dur == -1 {
            Component::translate("effect.duration.infinite", vec![])
        } else {
            Component::text(duration((dur as f32 * scale).floor() as i32))
        };
        line = Component::translate("potion.withDuration", vec![arg(line), arg(text)]);
    }
    let category = view.data.effect_colors.get(effect).map_or(0, |c| c >> 24);
    Some(colored(line, if category == 1 { 0xff_5555 } else { 0x55_55ff }))
}

/// Attribute modifiers carried by the effects: (effect, attribute, amount per level,
/// operation). The real amount is `amount × (amplifier + 1)`.
const EFFECT_MODS: [(&str, &str, f32, u8); 12] = [
    ("speed", "movement_speed", 0.2, 2),
    ("slowness", "movement_speed", -0.15, 2),
    ("haste", "attack_speed", 0.1, 2),
    ("mining_fatigue", "attack_speed", -0.1, 2),
    ("strength", "attack_damage", 3.0, 0),
    ("jump_boost", "safe_fall_distance", 1.0, 0),
    ("invisibility", "waypoint_transmit_range", -1.0, 2),
    ("weakness", "attack_damage", -4.0, 0),
    ("health_boost", "max_health", 4.0, 0),
    ("absorption", "max_absorption", 4.0, 0),
    ("luck", "luck", 1.0, 0),
    ("unluck", "luck", -1.0, 0),
];

/// Lines of an effect list: one per effect, "No Effects" if empty, then the
/// "When Applied" block of the attribute modifiers (effect order).
pub fn potion_lines(view: &StackRef<'_>, effects: &[(usize, i32, i32)], scale: f32, out: &mut Vec<Component>) {
    if effects.is_empty() {
        out.push(colored(Component::translate("effect.none", vec![]), 0xaa_aaaa));
    }
    let mut mods: Vec<(&str, f64, u8)> = Vec::new();
    for &e in effects {
        out.extend(effect_line(view, e, scale));
        let name = view.data.registry("minecraft:mob_effect").and_then(|r| r.entries.get(e.0));
        let path = name.map_or("", |n| n.strip_prefix("minecraft:").unwrap_or(n));
        for &(n, attr, amount, op) in &EFFECT_MODS {
            if n == path {
                mods.push((attr, f64::from(amount) * f64::from(e.1 + 1), op));
            }
        }
    }
    if mods.is_empty() {
        return;
    }
    out.push(Component::text(""));
    out.push(colored(Component::translate("potion.whenDrank", vec![]), 0xaa_00aa));
    for (attr, amount, op) in mods {
        let shown = if op == 1 || op == 2 { amount * 100.0 } else { amount };
        let desc = Component::translate(format!("attribute.name.{attr}"), vec![]);
        if amount > 0.0 {
            out.push(colored(
                Component::translate(format!("attribute.modifier.plus.{op}"), vec![Arg::Text(num(shown)), arg(desc)]),
                0x55_55ff,
            ));
        } else if amount < 0.0 {
            out.push(colored(
                Component::translate(format!("attribute.modifier.take.{op}"), vec![Arg::Text(num(-shown)), arg(desc)]),
                0xff_5555,
            ));
        }
    }
}

/// Attribute modifier read from the network: (attribute, id, value, operation, group, display).
type NetMod = (usize, String, f64, u8, u8, u8);

fn attribute_mods(b: &[u8]) -> Option<(Vec<NetMod>, Vec<Option<Component>>)> {
    let mut r = Reader::new(b);
    let n = read_varint(&mut r).ok()?;
    let mut out = Vec::new();
    let mut overrides = Vec::new();
    for _ in 0..n {
        let attr = read_varint(&mut r).ok()? as usize;
        let id = rc_proto::read_str(&mut r, 32767).ok()?.to_owned();
        let amount = f64::from_be_bytes(r.take(8).ok()?.try_into().ok()?);
        let op = read_varint(&mut r).ok()? as u8;
        let slot = read_varint(&mut r).ok()? as u8;
        let kind = read_varint(&mut r).ok()? as u8;
        let mut text = None;
        if kind == 2 {
            text = rc_proto::nbt::read(&mut r).ok()?.and_then(|n| rc_text::from_nbt(&n));
        }
        out.push((attr, id, amount, op, slot, kind));
        overrides.push(text);
    }
    Some((out, overrides))
}

/// Modifier to display: attribute, id, value, operation, group, display kind,
/// forced text.
type ModLine = (String, String, f64, u8, u8, u8, Option<Component>);

/// Lines of the attribute modifiers by slot group ("When in Main Hand: …").
fn attributes(view: &StackRef<'_>, hidden: &[u16], out: &mut Vec<Component>) {
    if view.ids.of("minecraft:attribute_modifiers").is_some_and(|i| hidden.contains(&i)) {
        return;
    }
    let attrs = view.data.registry("minecraft:attribute");
    // Patch list, otherwise the item's default one.
    let name_of = |a: usize| attrs.and_then(|r| r.entries.get(a)).map(|n| n.strip_prefix("minecraft:").unwrap_or(n));
    let mods: Vec<ModLine> = match view.patch("minecraft:attribute_modifiers") {
        Some(b) => {
            let Some((list, over)) = attribute_mods(b) else { return };
            list.into_iter()
                .zip(over)
                .filter_map(|((a, id, amt, op, slot, kind), text)| {
                    Some((name_of(a)?.to_owned(), id, amt, op, slot, kind, text))
                })
                .collect()
        }
        None => {
            let removed = view.ids.of("minecraft:attribute_modifiers").is_some_and(|i| view.stack.removed.contains(&i));
            if removed {
                return;
            }
            view.info
                .map(|i| {
                    i.attributes
                        .iter()
                        .map(|m| (m.attribute.clone(), m.id.clone(), m.amount, m.operation, m.slot, 0, None))
                        .collect()
                })
                .unwrap_or_default()
        }
    };
    const GROUPS: [&str; 11] =
        ["any", "mainhand", "offhand", "hand", "feet", "legs", "chest", "head", "armor", "body", "saddle"];
    for (slot, group) in GROUPS.iter().enumerate() {
        let mut first = true;
        for (attr, id, amount, op, s, kind, text) in &mods {
            if usize::from(*s) != slot || *kind == 1 {
                continue;
            }
            if first {
                out.push(Component::text(""));
                out.push(colored(Component::translate(format!("item.modifiers.{group}"), vec![]), 0xaa_aaaa));
                first = false;
            }
            if let Some(t) = text {
                out.push(t.clone());
                continue;
            }
            // Base attack: added to the player's base value, displayed "=".
            let mut value = *amount;
            let base = match id.as_str() {
                "minecraft:base_attack_damage" => {
                    value += 1.0;
                    true
                }
                "minecraft:base_attack_speed" => {
                    value += 4.0;
                    true
                }
                _ => false,
            };
            let shown = if *op == 1 || *op == 2 {
                value * 100.0
            } else if attr == "knockback_resistance" {
                value * 10.0
            } else {
                value
            };
            let desc = Component::translate(format!("attribute.name.{attr}"), vec![]);
            // Sentiment: negative for burning time and falls, neutral for
            // gravity, scale and beacon ranges.
            let (pos_color, neg_color) = match attr.as_str() {
                "burning_time" | "fall_damage_multiplier" => (0xff_5555, 0x55_55ff),
                "gravity" | "scale" | "waypoint_transmit_range" | "waypoint_receive_range" => (0xaa_aaaa, 0xaa_aaaa),
                _ => (0x55_55ff, 0xff_5555),
            };
            if base {
                let mut c = Component::translate(
                    format!("attribute.modifier.equals.{op}"),
                    vec![Arg::Text(num(shown)), arg(desc)],
                );
                c.style.color = Some(0x00_aa00);
                let mut line = Component::text(" ");
                line.extra.push(c);
                out.push(line);
            } else if value > 0.0 {
                out.push(colored(
                    Component::translate(
                        format!("attribute.modifier.plus.{op}"),
                        vec![Arg::Text(num(shown)), arg(desc)],
                    ),
                    pos_color,
                ));
            } else if value < 0.0 {
                out.push(colored(
                    Component::translate(
                        format!("attribute.modifier.take.{op}"),
                        vec![Arg::Text(num(-shown)), arg(desc)],
                    ),
                    neg_color,
                ));
            }
        }
    }
}

/// Lines of the tooltip after the name (excluded), in the game's order.
pub fn details(view: &StackRef<'_>, names: &[String], single: &[&str], advanced: bool, out: &mut Vec<Component>) {
    use crate::tooltips_items as ti;
    use crate::tooltips_more as tm;
    let (_, hidden) = display(view);
    let shows = |name: &str| view.ids.of(name).is_none_or(|i| !hidden.contains(&i));
    // Visible component: value of the stack, otherwise the item's default (bytes given).
    let comp = |name: &str, default: Option<&'static [u8]>| -> Option<&[u8]> {
        if !shows(name) {
            return None;
        }
        if let Some(b) = view.patch(name) {
            return Some(b);
        }
        let id = view.ids.of(name)?;
        let has = view.info.is_some_and(|i| i.components.contains(&id)) && !view.stack.removed.contains(&id);
        if has { default } else { None }
    };
    let item = view.item_id().to_owned();
    // Item-specific lines, then the components in the game's order.
    ti::item_specific(&item, out);
    tm::painting(&item, comp("minecraft:painting/variant", None), view.data, out);
    if let Some(p) = comp("minecraft:tropical_fish/pattern", None) {
        tm::tropical_fish(
            p,
            comp("minecraft:tropical_fish/base_color", None),
            comp("minecraft:tropical_fish/pattern_color", None),
            out,
        );
    }
    if let Some(b) = comp("minecraft:instrument", None) {
        ti::instrument(b, out);
    }
    if let Some(b) = comp("minecraft:bees", Some(&[0])) {
        ti::bees(b, out);
    }
    if comp("minecraft:container_loot", None).is_some() {
        out.push(Component::translate("item.container.loot_table.unknown", vec![]));
    }
    if let Some(b) = comp("minecraft:container", None) {
        ti::container(view, b, out);
    }
    if let Some(b) = comp("minecraft:banner_patterns", None) {
        ti::banner_patterns(b, out);
    }
    if let Some(b) = comp("minecraft:pot_decorations", None) {
        ti::pot_decorations(view, b, out);
    }
    if let Some(b) = comp("minecraft:written_book_content", None) {
        ti::written_book(b, out);
    }
    if let Some(b) = comp("minecraft:charged_projectiles", None) {
        let sub = |nv: &StackRef<'_>, o: &mut Vec<Component>| details(nv, names, single, false, o);
        ti::charged_projectiles(view, b, &sub, out);
    }
    if let Some(b) = comp("minecraft:fireworks", Some(&[1, 0])) {
        ti::fireworks(b, out);
    }
    if let Some(b) = comp("minecraft:firework_explosion", None) {
        ti::firework_star(b, out);
    }
    // Potion: effects (duration × `potion_duration_scale`), "No Effects" without effect.
    if let Some(b) = comp("minecraft:potion_contents", Some(&[0, 0, 0, 0]))
        && let Some(effects) = potion_effects(view, b)
    {
        // Duration scale: the stack's, otherwise the default (arrow 1/8, lingering 1/4).
        let scale = view.patch("minecraft:potion_duration_scale").and_then(|b| b.try_into().ok()).map_or_else(
            || match item.as_str() {
                "minecraft:tipped_arrow" => 0.125,
                "minecraft:lingering_potion" => 0.25,
                _ => 1.0,
            },
            f32::from_be_bytes,
        );
        potion_lines(view, &effects, scale, out);
    }
    let song = item.strip_prefix("minecraft:music_disc_");
    if let Some(b) = comp("minecraft:jukebox_playable", None) {
        ti::jukebox_playable(b, out);
    } else if let Some(s) = song
        && shows("minecraft:jukebox_playable")
    {
        ti::disc_song(s, out);
    }
    if let Some(b) = comp("minecraft:trim", None) {
        ti::trim(b, out);
    }
    for name in ["minecraft:stored_enchantments", "minecraft:enchantments"] {
        if shows(name)
            && let Some(b) = view.patch(name)
        {
            enchantments(b, names, single, out);
        }
    }
    // Dye: "Dyed"; in advanced, the hexadecimal colour.
    if shows("minecraft:dyed_color")
        && let Some(b) = view.patch("minecraft:dyed_color")
        && let Ok(rgb) = <[u8; 4]>::try_from(b.get(..4).unwrap_or_default())
    {
        let rgb = u32::from_be_bytes(rgb) & 0xff_ffff;
        if advanced {
            out.push(colored(Component::translate("item.color", vec![Arg::Text(format!("#{rgb:06X}"))]), 0xaa_aaaa));
        } else {
            let mut c = colored(Component::translate("item.dyed", vec![]), 0xaa_aaaa);
            c.style.italic = Some(true);
            out.push(c);
        }
    }
    if let Some(b) = comp("minecraft:profile", None)
        && crate::item_special::profile_dynamic(b)
    {
        out.push(colored(Component::translate("component.profile.dynamic", vec![]), 0xaa_aaaa));
    }
    if shows("minecraft:lore")
        && let Some(b) = view.patch("minecraft:lore")
    {
        let mut r = Reader::new(b);
        let n = read_varint(&mut r).unwrap_or(0);
        for _ in 0..n {
            let Ok(Some(nbt)) = rc_proto::nbt::read(&mut r) else { break };
            let Some(c) = rc_text::from_nbt(&nbt) else { continue };
            let mut line = Component::text("");
            line.style.color = Some(0xaa_00aa);
            line.style.italic = Some(true);
            line.extra.push(c);
            out.push(line);
        }
    }
    // Absorbed contents of a sulfur cube.
    if let Some(b) = comp("minecraft:sulfur_cube_content", None)
        && let Ok(st) = view.ids.codec.read_template(&mut Reader::new(b))
    {
        let mut line = Component::translate(
            "entity.minecraft.sulfur_cube.content",
            vec![arg(ti::hover_name(&ti::nested(view, &st)))],
        );
        line.style.italic = Some(true);
        out.push(colored(line, 0xaa_aaaa));
    }
    attributes(view, &hidden, out);
    if comp("minecraft:intangible_projectile", None).is_some() {
        out.push(colored(Component::translate("item.intangible", vec![]), 0xaa_aaaa));
    }
    if shows("minecraft:unbreakable") && view.patch("minecraft:unbreakable").is_some() {
        out.push(colored(Component::translate("item.unbreakable", vec![]), 0x55_55ff));
    }
    for (name, front) in [("minecraft:sign_text_front", true), ("minecraft:sign_text_back", false)] {
        if let Some(b) = comp(name, None) {
            tm::sign_text(front, b, out);
        }
    }
    if let Some(b) = comp("minecraft:ominous_bottle_amplifier", Some(&[0]))
        && let Ok(amp) = read_varint(&mut Reader::new(b))
        && let Some(bad_omen) = view
            .data
            .registry("minecraft:mob_effect")
            .and_then(|r| r.entries.iter().position(|n| n.strip_prefix("minecraft:").unwrap_or(n) == "bad_omen"))
    {
        potion_lines(view, &[(bad_omen, amp, 120_000)], 1.0, out);
    }
    if let Some(b) = comp("minecraft:suspicious_stew_effects", None) {
        let effects = tm::stew_effects(b);
        if !effects.is_empty() {
            potion_lines(view, &effects, 1.0, out);
        }
    }
    if let Some(b) = comp("minecraft:block_state", None) {
        ti::block_state(b, out);
    }
    let (peaceful, operator) = context();
    peaceful_line(view, peaceful, shows("minecraft:entity_data"), out);
    if shows("minecraft:block_entity_data") {
        tm::spawner(&item, view.patch("minecraft:block_entity_data"), out);
    }
    for (name, header) in [("minecraft:can_break", "item.canBreak"), ("minecraft:can_place_on", "item.canPlace")] {
        if let Some(b) = comp(name, None) {
            ti::block_predicates(view, header, b, out);
        }
    }
    if advanced {
        let max = view.max_damage();
        let damage = view.damage();
        if max > 0 && damage > 0 && shows("minecraft:damage") {
            out.push(Component::translate(
                "item.durability",
                vec![Arg::Text((max - damage).to_string()), Arg::Text(max.to_string())],
            ));
        }
        out.push(colored(Component::text(view.item_id()), 0x55_5555));
        // Merged components: the item's, plus the added ones, minus the removed ones.
        let mut set: HashSet<u16> = view.info.map(|i| i.components.iter().copied().collect()).unwrap_or_default();
        set.extend(view.stack.added.iter().map(|c| c.0));
        for r in &view.stack.removed {
            set.remove(r);
        }
        if !set.is_empty() {
            out.push(colored(
                Component::translate("item.components", vec![Arg::Text(set.len().to_string())]),
                0x55_5555,
            ));
        }
    }
    // "item.disabled": no item of game 26.3 requires an experimental feature, the
    // line never appears (conforming without code).
    op_warning(view, operator, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_like_the_game() {
        assert_eq!(num(7.0), "7");
        assert_eq!(num(1.6), "1.6");
        assert_eq!(num(-2.4000000953674316 + 4.0), "1.6");
        assert_eq!(num(0.125), "0.12");
        assert_eq!(num(20.0), "20");
    }

    fn keys(out: &[Component]) -> Vec<String> {
        out.iter()
            .filter_map(|c| match &c.content {
                rc_text::Content::Translate { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect()
    }

    /// Eggs in peaceful and operator warning, from the game's data.
    #[test]
    fn peaceful_and_operator_lines() {
        use rc_proto_v777::item::ItemStack;
        let data = rc_testdata::version_data().expect("version data missing (extraction)");
        let ids = crate::stack::ComponentIds::new(&data);
        let reg = |r: &str, n: &str| -> usize {
            data.registry(r).and_then(|r| r.entries.iter().position(|e| e == n)).expect(n)
        };
        let typed = |r: &str, n: &str| {
            let mut v = Vec::new();
            rc_proto::varint::write_varint(&mut v, reg(r, n) as i32);
            v.extend_from_slice(&[10, 0]);
            v
        };
        let stack = |item: &str, added: Vec<(u16, Vec<u8>)>| ItemStack {
            item: reg("minecraft:item", item) as i32,
            count: 1,
            added: added.into_iter().map(|(i, b)| (i, b.into())).collect(),
            removed: Vec::new(),
        };
        let view = |st: &'_ ItemStack| -> Vec<String> {
            let info = data.items.get(st.item as usize);
            let v = StackRef { stack: st, info, ids: &ids, data: &data, hold: crate::stack::Hold::NONE };
            let mut out = Vec::new();
            peaceful_line(&v, true, true, &mut out);
            op_warning(&v, true, &mut out);
            keys(&out)
        };
        let peaceful = |st: &ItemStack, on: bool| {
            let v = StackRef { stack: st, info: data.items.get(st.item as usize), ids: &ids, data: &data, hold: crate::stack::Hold::NONE };
            let mut out = Vec::new();
            peaceful_line(&v, on, true, &mut out);
            keys(&out)
        };
        let zombie = stack("minecraft:zombie_spawn_egg", vec![]);
        assert_eq!(view(&zombie), vec!["item.spawn_egg.peaceful"]);
        assert!(peaceful(&zombie, false).is_empty());
        assert!(view(&stack("minecraft:cow_spawn_egg", vec![])).is_empty());
        // Cow egg set to a zombie: the type of the data counts.
        let ed = ids.of("minecraft:entity_data").expect("entity_data");
        let cow_zombie = stack("minecraft:cow_spawn_egg", vec![(ed, typed("minecraft:entity_type", "minecraft:zombie"))]);
        assert_eq!(peaceful(&cow_zombie, true), vec!["item.spawn_egg.peaceful"]);
        // Data reserved for operators.
        let falling = stack("minecraft:cow_spawn_egg", vec![(ed, typed("minecraft:entity_type", "minecraft:falling_block"))]);
        assert_eq!(view(&falling), vec!["item.op_warning.line1", "item.op_warning.line2", "item.op_warning.line3"]);
        let bed = ids.of("minecraft:block_entity_data").expect("block_entity_data");
        let spawner = typed("minecraft:block_entity_type", "minecraft:mob_spawner");
        assert_eq!(view(&stack("minecraft:stone", vec![(bed, spawner.clone())])).len(), 3);
        assert!(view(&stack("minecraft:paper", vec![(bed, spawner)])).is_empty());
        let chest = typed("minecraft:block_entity_type", "minecraft:chest");
        assert!(view(&stack("minecraft:chest", vec![(bed, chest)])).is_empty());
        let v = StackRef { stack: &falling, info: None, ids: &ids, data: &data, hold: crate::stack::Hold::NONE };
        let mut out = Vec::new();
        op_warning(&v, false, &mut out);
        assert!(out.is_empty(), "non-operator player");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(3600), "03:00");
        assert_eq!(duration(1800), "01:30");
        assert_eq!(duration(20 * 3600 * 2), "02:00:00");
    }
}
