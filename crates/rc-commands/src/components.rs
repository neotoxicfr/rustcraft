//! Item component values given in SNBT (`item[component=value]`): checked against the
//! component's expected format, with the game decoder's error message (rendered in
//! `arguments.item.component.malformed`). Only components with a simple value (bounded integer,
//! bounded real, boolean, enumeration, identifier) are checked; compounds stay accepted.

use crate::snbt::Value;

/// Components with no saved form: refused as unknown in commands.
pub const TRANSIENT: [&str; 3] =
    ["minecraft:creative_slot_lock", "minecraft:map_post_processing", "minecraft:additional_trade_cost"];

pub fn is_transient(id: &str) -> bool {
    TRANSIENT.contains(&id)
}

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
const RARITIES: [&str; 4] = ["common", "uncommon", "rare", "epic"];

/// SNBT number read from its text (sign, base 16 or 2, `_`, type suffixes).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Num {
    Int(i64),
    Real(f64),
}

fn number_text(t: &str) -> Option<Num> {
    let t: String = t.trim().chars().filter(|&c| c != '_').collect();
    let lower = t.to_ascii_lowercase();
    let (neg, body) = match lower.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, lower.strip_prefix('+').unwrap_or(&lower)),
    };
    let int_suffix = |s: &str| -> (String, Option<char>, bool) {
        for (suf, ty, unsigned) in
            [("ub", 'b', true), ("us", 's', true), ("ui", 'i', true), ("ul", 'l', true)]
                .into_iter()
                .chain([("sb", 'b', false), ("ss", 's', false), ("si", 'i', false), ("sl", 'l', false)])
        {
            if let Some(d) = s.strip_suffix(suf) {
                return (d.to_owned(), Some(ty), unsigned);
            }
        }
        match s.chars().last() {
            Some(c @ ('b' | 's' | 'i' | 'l')) => (s[..s.len() - 1].to_owned(), Some(c), false),
            _ => (s.to_owned(), None, false),
        }
    };
    let as_int = |digits: &str, radix: u32| -> Option<Num> {
        let (d, ty, unsigned) = int_suffix(digits);
        let v = i128::from_str_radix(&d, radix).ok()?;
        let v = if neg { -v } else { v };
        let v = match (ty, unsigned) {
            (Some('b'), true) => i64::from(v as u8 as i8),
            (Some('b'), false) => i64::from(v as i8),
            (Some('s'), true) => i64::from(v as u16 as i16),
            (Some('s'), false) => i64::from(v as i16),
            (Some('l'), _) => v as i64,
            (_, true) => i64::from(v as u32 as i32),
            _ => i64::from(v as i32),
        };
        Some(Num::Int(v))
    };
    if let Some(h) = body.strip_prefix("0x") {
        return as_int(h, 16);
    }
    if let Some(b) = body.strip_prefix("0b")
        && b.starts_with(['0', '1'])
    {
        return as_int(b, 2);
    }
    if body.contains(['.', 'e', 'f', 'd']) && !body.ends_with(['b', 's', 'i', 'l']) {
        let f: f64 = body.trim_end_matches(['f', 'd']).parse().ok()?;
        let f = if neg { -f } else { f };
        // Simple real (`f`) rounded to f32 like the game's tag.
        return Some(Num::Real(if body.ends_with('f') { f64::from(f as f32) } else { f }));
    }
    as_int(body, 10)
}

/// Numeric reading of a tag by the decoder (booleans' bytes included).
fn number(v: &Value, text: &str) -> Result<Num, String> {
    match v {
        Value::Bool(b) => Ok(Num::Int(i64::from(*b))),
        Value::Number | Value::Float => number_text(text).ok_or_else(|| "Not a number".to_owned()),
        _ => Err("Not a number".to_owned()),
    }
}

/// Integer value of a number (Java's integer conversion: truncation, saturation of reals).
fn int_of(n: Num) -> i32 {
    match n {
        Num::Int(i) => i as i32,
        Num::Real(f) => f as i32,
    }
}

fn float_of(n: Num) -> f32 {
    match n {
        Num::Int(i) => i as f32,
        Num::Real(f) => f as f32,
    }
}

fn string(v: &Value) -> Result<&str, String> {
    match v {
        Value::Str(s) => Ok(s),
        _ => Err("Not a string".to_owned()),
    }
}

fn int_range(v: &Value, text: &str, min: i32, max: i32, msg: impl Fn(i32) -> String) -> Result<(), String> {
    let n = int_of(number(v, text)?);
    if (min..=max).contains(&n) { Ok(()) } else { Err(msg(n)) }
}

fn ns_char(c: char) -> bool {
    matches!(c, '_' | '-' | 'a'..='z' | '0'..='9' | '.')
}

fn path_char(c: char) -> bool {
    ns_char(c) || c == '/'
}

/// Decoder identifier: namespace then path checked, game message.
fn identifier(s: &str) -> Result<(), String> {
    let (ns, path) = match s.find(':') {
        Some(0) => ("minecraft", &s[1..]),
        Some(i) => (&s[..i], &s[i + 1..]),
        None => ("minecraft", s),
    };
    let bad = if ns == ".." || !ns.chars().all(ns_char) {
        Some(format!("Non [a-z0-9_.-] character in namespace of identifier: {ns}:{path}"))
    } else if !path.chars().all(path_char) {
        Some(format!("Non [a-z0-9/._-] character in path of location: {ns}:{path}"))
    } else {
        None
    };
    match bad {
        Some(e) => Err(format!("Not a valid resource location: {s} {e}")),
        None => Ok(()),
    }
}

fn enumerated(v: &Value, names: &[&str]) -> Result<(), String> {
    let s = string(v)?;
    if names.contains(&s) { Ok(()) } else { Err(format!("Unknown element name:{s}")) }
}

/// Check of a component value (`text`: its written form); `Err` = decoder message.
pub fn check(id: &str, v: &Value, text: &str) -> Result<(), String> {
    let short = id.strip_prefix("minecraft:").unwrap_or(id);
    let non_negative = |n: i32| format!("Value must be non-negative: {n}");
    match short {
        "max_stack_size" => int_range(v, text, 1, 99, |n| format!("Value must be within range [1;99]: {n}")),
        "max_damage" => int_range(v, text, 1, i32::MAX, |n| format!("Value must be positive: {n}")),
        "damage" | "repair_cost" => int_range(v, text, 0, i32::MAX, non_negative),
        "map_id" => number(v, text).map(|_| ()),
        "enchantment_glint_override" => number(v, text).map(|_| ()),
        "minimum_attack_charge" => {
            let f = float_of(number(v, text)?);
            if (0.0..=1.0).contains(&f) {
                Ok(())
            } else {
                Err(format!("Value must be within range [0.0;1.0]: {}", crate::javafmt::jf(f)))
            }
        }
        "potion_duration_scale" => {
            let f = float_of(number(v, text)?);
            if (0.0..=f32::MAX).contains(&f) {
                Ok(())
            } else {
                Err(format!("Value must be non-negative: {}", crate::javafmt::jf(f)))
            }
        }
        "rarity" => enumerated(v, &RARITIES),
        "dye" | "base_color" | "wolf/collar" | "tropical_fish/base_color" | "tropical_fish/pattern_color" => {
            enumerated(v, &DYES)
        }
        "item_model" | "tooltip_style" | "note_block_sound" => identifier(string(v)?),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snbt_numbers() {
        assert_eq!(number_text("5"), Some(Num::Int(5)));
        assert_eq!(number_text("-1b"), Some(Num::Int(-1)));
        assert_eq!(number_text("255ub"), Some(Num::Int(-1)));
        assert_eq!(number_text("0x1F"), Some(Num::Int(31)));
        assert_eq!(number_text("1_000"), Some(Num::Int(1000)));
        assert_eq!(number_text("1.5f"), Some(Num::Real(1.5)));
        assert_eq!(number_text("2d"), Some(Num::Real(2.0)));
    }

    #[test]
    fn decoder_messages() {
        let s = |t: &str| Value::Str(t.to_owned());
        assert_eq!(check("minecraft:damage", &s("x"), "\"x\""), Err("Not a number".into()));
        assert_eq!(check("minecraft:damage", &Value::Number, "-1"), Err("Value must be non-negative: -1".into()));
        assert_eq!(check("minecraft:damage", &Value::Number, "3"), Ok(()));
        assert_eq!(
            check("minecraft:max_stack_size", &Value::Number, "100"),
            Err("Value must be within range [1;99]: 100".into())
        );
        assert_eq!(check("minecraft:max_damage", &Value::Number, "0"), Err("Value must be positive: 0".into()));
        assert_eq!(
            check("minecraft:minimum_attack_charge", &Value::Float, "1.5f"),
            Err("Value must be within range [0.0;1.0]: 1.5".into())
        );
        assert_eq!(check("minecraft:rarity", &s("legendary"), ""), Err("Unknown element name:legendary".into()));
        assert_eq!(check("minecraft:rarity", &Value::Number, "1"), Err("Not a string".into()));
        assert_eq!(check("minecraft:dye", &s("red"), ""), Ok(()));
        assert_eq!(
            check("minecraft:item_model", &s("Diamond"), ""),
            Err("Not a valid resource location: Diamond Non [a-z0-9/._-] character in path of location: minecraft:Diamond"
                .into())
        );
        assert_eq!(check("minecraft:custom_data", &s("x"), ""), Ok(()));
    }
}
