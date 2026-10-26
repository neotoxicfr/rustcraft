//! Command text NBT: full reading (typed values, `commands.md` § 2.4.1),
//! compact writing (conversion to the game's text) and colored rendering of `data get` (cyan keys, green
//! strings, gold numbers, red suffixes; key order of a Java hash table).

use rc_proto::nbt::Nbt;
use rc_text::Component;

use super::text::{AQUA, GOLD, GREEN, RED, colored, join, lit};

pub struct P<'a> {
    pub s: &'a [char],
    pub i: usize,
}

fn ws(c: char) -> bool {
    c.is_whitespace()
}

fn unq(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')
}

impl P<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && ws(self.s[self.i]) {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn eat(&mut self, c: char) -> bool {
        self.skip();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self) -> String {
        let st = self.i;
        while self.i < self.s.len() && unq(self.s[self.i]) {
            self.i += 1;
        }
        self.s[st..self.i].iter().collect()
    }

    pub fn value(&mut self) -> Result<Nbt, &'static str> {
        self.skip();
        match self.peek() {
            None => Err("snbt.parser.expected_unquoted_string"),
            Some('{') => self.compound(),
            Some('[') => self.list(),
            Some('"' | '\'') => Ok(Nbt::String(self.quoted()?)),
            Some(_) => self.scalar(),
        }
    }

    pub fn quoted(&mut self) -> Result<String, &'static str> {
        self.skip();
        let q = self.s[self.i];
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else { return Err("parsing.quote.expected.end") };
            self.i += 1;
            if c == q {
                return Ok(out);
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let Some(e) = self.peek() else { return Err("parsing.quote.expected.end") };
            self.i += 1;
            match e {
                'b' => out.push('\u{8}'),
                's' => out.push(' '),
                't' => out.push('\t'),
                'n' => out.push('\n'),
                'f' => out.push('\u{c}'),
                'r' => out.push('\r'),
                '\\' | '\'' | '"' => out.push(e),
                'x' | 'u' | 'U' => {
                    let n = match e {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let h: String = self.s.get(self.i..self.i + n).map(|v| v.iter().collect()).unwrap_or_default();
                    if h.chars().count() != n || !h.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Err("snbt.parser.expected_hex_escape");
                    }
                    self.i += n;
                    let v = u32::from_str_radix(&h, 16).map_err(|_| "snbt.parser.invalid_codepoint")?;
                    out.push(char::from_u32(v).ok_or("snbt.parser.invalid_codepoint")?);
                }
                // `\N{name}`: character designated by its Unicode name (case-insensitive).
                'N' if self.peek() == Some('{') => {
                    self.i += 1;
                    let st = self.i;
                    while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ' ')) {
                        self.i += 1;
                    }
                    if self.i == st || self.peek() != Some('}') {
                        return Err("snbt.parser.invalid_character_name");
                    }
                    let name: String = self.s[st..self.i].iter().collect();
                    self.i += 1;
                    out.push(char_by_name(&name).ok_or("snbt.parser.invalid_character_name")?);
                }
                _ => return Err("snbt.parser.invalid_string_contents"),
            }
        }
    }

    fn compound(&mut self) -> Result<Nbt, &'static str> {
        self.i += 1;
        let mut f: Vec<(String, Nbt)> = Vec::new();
        loop {
            self.skip();
            if self.eat('}') {
                return Ok(Nbt::Compound(f));
            }
            self.skip();
            let k = match self.peek() {
                Some('"' | '\'') => {
                    let k = self.quoted()?;
                    if k.is_empty() {
                        return Err("snbt.parser.empty_key");
                    }
                    k
                }
                _ => {
                    let w = self.word();
                    if w.is_empty() {
                        return Err("snbt.parser.expected_unquoted_string");
                    }
                    w
                }
            };
            if !self.eat(':') {
                return Err("snbt.parser.expected_unquoted_string");
            }
            let v = self.value()?;
            f.retain(|(o, _)| *o != k);
            f.push((k, v));
            if !self.eat(',') {
                if self.eat('}') {
                    return Ok(Nbt::Compound(f));
                }
                return Err("snbt.parser.expected_unquoted_string");
            }
        }
    }

    fn list(&mut self) -> Result<Nbt, &'static str> {
        self.i += 1;
        self.skip();
        // Typed array `[B;`, `[I;`, `[L;`.
        if let (Some(t), Some(';')) = (self.peek(), self.s.get(self.i + 1).copied())
            && matches!(t, 'B' | 'I' | 'L')
        {
            self.i += 2;
            // Accepted element types: byte alone; int, byte, short; long, byte, short, int.
            let allowed: &[char] = match t {
                'B' => &['b'],
                'I' => &['i', 'b', 's'],
                _ => &['l', 'b', 's', 'i'],
            };
            let mut vals: Vec<i64> = Vec::new();
            loop {
                self.skip();
                if self.eat(']') {
                    break;
                }
                self.skip();
                let w = self.word();
                let lit = int_literal(&w)?;
                let ty = lit.ty.unwrap_or(allowed[0]);
                if !allowed.contains(&ty) {
                    return Err("snbt.parser.invalid_array_element_type");
                }
                let v = int_value(&lit, ty)?;
                vals.push(v.as_i64().unwrap_or(0));
                if !self.eat(',') {
                    if self.eat(']') {
                        break;
                    }
                    return Err("snbt.parser.expected_unquoted_string");
                }
            }
            return Ok(match t {
                'B' => Nbt::ByteArray(vals.iter().map(|&v| v as u8).collect()),
                'I' => Nbt::IntArray(vals.iter().map(|&v| v as i32).collect()),
                _ => Nbt::LongArray(vals),
            });
        }
        let mut items = Vec::new();
        loop {
            self.skip();
            if self.eat(']') {
                break;
            }
            items.push(self.value()?);
            if !self.eat(',') {
                if self.eat(']') {
                    break;
                }
                return Err("snbt.parser.expected_unquoted_string");
            }
        }
        // List kept as is in memory, even heterogeneous (the `{"": x}` wrapper only exists
        // at binary writing).
        Ok(Nbt::List(items))
    }

    fn scalar(&mut self) -> Result<Nbt, &'static str> {
        self.skip();
        let st = self.i;
        let w = self.word();
        if w.is_empty() {
            return Err("snbt.parser.expected_unquoted_string");
        }
        let c0 = w.chars().next().unwrap_or(' ');
        if c0.is_ascii_digit() || matches!(c0, '+' | '-' | '.') {
            return match number(&w) {
                Some(n) => n,
                None => {
                    self.i = st;
                    Err("snbt.parser.invalid_unquoted_start")
                }
            };
        }
        // Built-in operations `name(arguments…)`: `bool/1`, `uuid/1`; any other name is refused.
        if self.peek() == Some('(') {
            self.i += 1;
            let mut args = Vec::new();
            loop {
                if self.eat(')') {
                    break;
                }
                args.push(self.value()?);
                if !self.eat(',') {
                    if self.eat(')') {
                        break;
                    }
                    return Err("snbt.parser.expected_unquoted_string");
                }
            }
            return match (w.as_str(), args.as_slice()) {
                ("bool", [arg]) => {
                    let x = match arg {
                        Nbt::Byte(_) | Nbt::Short(_) | Nbt::Int(_) | Nbt::Long(_) | Nbt::Float(_) | Nbt::Double(_) => {
                            num_f64(arg)
                        }
                        _ => return Err("snbt.parser.expected_number_or_boolean"),
                    };
                    Ok(Nbt::Byte(i8::from(x != 0.0)))
                }
                ("uuid", [arg]) => {
                    let s = arg.as_str().ok_or("snbt.parser.expected_string_uuid")?;
                    let u = parse_uuid(s).ok_or("snbt.parser.expected_string_uuid")?;
                    Ok(crate::nbtio::uuid_nbt(u))
                }
                _ => Err("snbt.parser.no_such_operation"),
            };
        }
        if w.eq_ignore_ascii_case("true") {
            return Ok(Nbt::Byte(1));
        }
        if w.eq_ignore_ascii_case("false") {
            return Ok(Nbt::Byte(0));
        }
        Ok(Nbt::String(w))
    }
}

/// Text identifier `a-b-c-d-e` (five hexadecimal groups, sign allowed, 36 characters at most,
/// each group within the bounds of a signed long, then truncated to its width: 32, 16, 16, 16, 48 bits).
pub fn parse_uuid(s: &str) -> Option<u128> {
    if s.chars().count() > 36 {
        return None;
    }
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return None;
    }
    let mut out: u128 = 0;
    for (p, bits) in parts.iter().zip([32u32, 16, 16, 16, 48]) {
        let (neg, d) = match p.as_bytes().first() {
            Some(b'-') => (true, &p[1..]),
            Some(b'+') => (false, &p[1..]),
            _ => (false, *p),
        };
        if d.is_empty() || !d.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let m = u128::from_str_radix(d, 16).ok()?;
        let v = if neg { -i128::try_from(m).ok()? } else { i128::try_from(m).ok()? };
        let v = i64::try_from(v).ok()?;
        out = out << bits | (u128::from(v as u64) & ((1u128 << bits) - 1));
    }
    Some(out)
}

/// Character designated by its Unicode name: case-insensitive, edge whitespace ignored, aliases
/// do not count (only the character's proper name is recognized).
/// Code points assigned without a proper name in Java's name table: it only knows them
/// in the form "BLOCK HEX" (`CJK UNIFIED IDEOGRAPHS 4E00`) and refuses their constructed names
/// (`HANGUL SYLLABLE GA`). Taken from the Java 25 runtime shipped with the game (surrogates,
/// impossible in a Rust string, stay refused).
const JAVA_UNNAMED: [(u32, u32, &str); 18] = [
    (0x84, 0x84, "LATIN 1 SUPPLEMENT"),
    (0x3400, 0x4DBF, "CJK UNIFIED IDEOGRAPHS EXTENSION A"),
    (0x4E00, 0x9FFF, "CJK UNIFIED IDEOGRAPHS"),
    (0xAC00, 0xD7A3, "HANGUL SYLLABLES"),
    (0xE000, 0xF8FF, "PRIVATE USE AREA"),
    (0x17000, 0x187F7, "TANGUT"),
    (0x18D00, 0x18D08, "TANGUT SUPPLEMENT"),
    (0x20000, 0x2A6DF, "CJK UNIFIED IDEOGRAPHS EXTENSION B"),
    (0x2A700, 0x2B739, "CJK UNIFIED IDEOGRAPHS EXTENSION C"),
    (0x2B740, 0x2B81D, "CJK UNIFIED IDEOGRAPHS EXTENSION D"),
    (0x2B820, 0x2CEA1, "CJK UNIFIED IDEOGRAPHS EXTENSION E"),
    (0x2CEB0, 0x2EBE0, "CJK UNIFIED IDEOGRAPHS EXTENSION F"),
    (0x2EBF0, 0x2EE5D, "CJK UNIFIED IDEOGRAPHS EXTENSION I"),
    (0x30000, 0x3134A, "CJK UNIFIED IDEOGRAPHS EXTENSION G"),
    (0x31350, 0x323AF, "CJK UNIFIED IDEOGRAPHS EXTENSION H"),
    (0xF0000, 0xFFFFD, "SUPPLEMENTARY PRIVATE USE AREA A"),
    (0x100000, 0x10FFFD, "SUPPLEMENTARY PRIVATE USE AREA B"),
    (0xD800, 0xDFFF, "SURROGATES"),
];

/// Control characters that Java's table names (names of the old standard) and that
/// `unicode_names2` does not name; those whose name carries parentheses (`LINE FEED (LF)`)
/// are outside the grammar of `\N{…}` and omitted.
const JAVA_CONTROLS: [(u32, &str); 60] = [
    (0x0, "NULL"),
    (0x1, "START OF HEADING"),
    (0x2, "START OF TEXT"),
    (0x3, "END OF TEXT"),
    (0x4, "END OF TRANSMISSION"),
    (0x5, "ENQUIRY"),
    (0x6, "ACKNOWLEDGE"),
    (0x7, "BEL"),
    (0x8, "BACKSPACE"),
    (0x9, "CHARACTER TABULATION"),
    (0xB, "LINE TABULATION"),
    (0xE, "SHIFT OUT"),
    (0xF, "SHIFT IN"),
    (0x10, "DATA LINK ESCAPE"),
    (0x11, "DEVICE CONTROL ONE"),
    (0x12, "DEVICE CONTROL TWO"),
    (0x13, "DEVICE CONTROL THREE"),
    (0x14, "DEVICE CONTROL FOUR"),
    (0x15, "NEGATIVE ACKNOWLEDGE"),
    (0x16, "SYNCHRONOUS IDLE"),
    (0x17, "END OF TRANSMISSION BLOCK"),
    (0x18, "CANCEL"),
    (0x19, "END OF MEDIUM"),
    (0x1A, "SUBSTITUTE"),
    (0x1B, "ESCAPE"),
    (0x1C, "INFORMATION SEPARATOR FOUR"),
    (0x1D, "INFORMATION SEPARATOR THREE"),
    (0x1E, "INFORMATION SEPARATOR TWO"),
    (0x1F, "INFORMATION SEPARATOR ONE"),
    (0x7F, "DELETE"),
    (0x80, "PADDING CHARACTER"),
    (0x81, "HIGH OCTET PRESET"),
    (0x82, "BREAK PERMITTED HERE"),
    (0x83, "NO BREAK HERE"),
    (0x86, "START OF SELECTED AREA"),
    (0x87, "END OF SELECTED AREA"),
    (0x88, "CHARACTER TABULATION SET"),
    (0x89, "CHARACTER TABULATION WITH JUSTIFICATION"),
    (0x8A, "LINE TABULATION SET"),
    (0x8B, "PARTIAL LINE FORWARD"),
    (0x8C, "PARTIAL LINE BACKWARD"),
    (0x8D, "REVERSE LINE FEED"),
    (0x8E, "SINGLE SHIFT TWO"),
    (0x8F, "SINGLE SHIFT THREE"),
    (0x90, "DEVICE CONTROL STRING"),
    (0x91, "PRIVATE USE ONE"),
    (0x92, "PRIVATE USE TWO"),
    (0x93, "SET TRANSMIT STATE"),
    (0x94, "CANCEL CHARACTER"),
    (0x95, "MESSAGE WAITING"),
    (0x96, "START OF GUARDED AREA"),
    (0x97, "END OF GUARDED AREA"),
    (0x98, "START OF STRING"),
    (0x99, "SINGLE GRAPHIC CHARACTER INTRODUCER"),
    (0x9A, "SINGLE CHARACTER INTRODUCER"),
    (0x9B, "CONTROL SEQUENCE INTRODUCER"),
    (0x9C, "STRING TERMINATOR"),
    (0x9D, "OPERATING SYSTEM COMMAND"),
    (0x9E, "PRIVACY MESSAGE"),
    (0x9F, "APPLICATION PROGRAM COMMAND"),
];

/// Code points added after the Unicode version of Java 25 (`unicode_names2` 4 names them,
/// Java considers them unassigned and refuses their name).
const JAVA_UNASSIGNED: [(u32, u32); 45] = [
    (0x88F, 0x88F),
    (0xC5C, 0xC5C),
    (0xCDC, 0xCDC),
    (0x1ACF, 0x1ADD),
    (0x1AE0, 0x1AEB),
    (0x20C1, 0x20C1),
    (0x2B96, 0x2B96),
    (0xA7CE, 0xA7CF),
    (0xA7D2, 0xA7D2),
    (0xA7D4, 0xA7D4),
    (0xA7F1, 0xA7F1),
    (0xFBC3, 0xFBD2),
    (0xFD90, 0xFD91),
    (0xFDC8, 0xFDCE),
    (0x10940, 0x10959),
    (0x10EC5, 0x10EC7),
    (0x10ED0, 0x10ED8),
    (0x10EFA, 0x10EFB),
    (0x11B60, 0x11B67),
    (0x11DB0, 0x11DDB),
    (0x11DE0, 0x11DE9),
    (0x16EA0, 0x16EB8),
    (0x16EBB, 0x16ED3),
    (0x16FF2, 0x16FF6),
    (0x18D80, 0x18DF2),
    (0x1CCFA, 0x1CCFC),
    (0x1CEBA, 0x1CED0),
    (0x1CEE0, 0x1CEF0),
    (0x1E6C0, 0x1E6DE),
    (0x1E6E0, 0x1E6F5),
    (0x1E6FE, 0x1E6FF),
    (0x1F6D8, 0x1F6D8),
    (0x1F777, 0x1F77A),
    (0x1F8D0, 0x1F8D8),
    (0x1FA54, 0x1FA57),
    (0x1FA8A, 0x1FA8A),
    (0x1FA8E, 0x1FA8E),
    (0x1FAC8, 0x1FAC8),
    (0x1FACD, 0x1FACD),
    (0x1FAEA, 0x1FAEA),
    (0x1FAEF, 0x1FAEF),
    (0x1FBFA, 0x1FBFA),
    (0x2B73A, 0x2B73F),
    (0x2CEA2, 0x2CEAD),
    (0x323B0, 0x33479),
];

/// Lookup by name like Java: case-insensitive, edge whitespace removed; exact proper name
/// (no alias), or "BLOCK HEX" (uppercase hexadecimal without leading zero) for a code
/// point without a proper name. Java 25 table (Unicode 16): named controls, additions of Unicode 17
/// unknown.
fn char_by_name(name: &str) -> Option<char> {
    let n = name.trim_matches(|c: char| c <= ' ').to_ascii_uppercase();
    if let Some(&(cp, _)) = JAVA_CONTROLS.iter().find(|c| c.1 == n) {
        return char::from_u32(cp);
    }
    let unnamed = |cp: u32| {
        JAVA_UNNAMED
            .iter()
            .find(|r| (r.0..=r.1).contains(&cp))
            .map(|r| r.2)
            .or_else(|| JAVA_UNASSIGNED.iter().any(|r| (r.0..=r.1).contains(&cp)).then_some(""))
    };
    if let Some((block, hex)) = n.rsplit_once(' ')
        && let Ok(cp) = u32::from_str_radix(hex, 16)
        && unnamed(cp).is_some_and(|b| !b.is_empty() && b == block)
        && format!("{cp:X}") == hex
    {
        return char::from_u32(cp);
    }
    let c = unicode_names2::character(&n)?;
    (unnamed(c as u32).is_none() && unicode_names2::name(c)?.to_string() == n).then_some(c)
}

fn num_f64(v: &Nbt) -> f64 {
    match *v {
        Nbt::Float(f) => f64::from(f),
        Nbt::Double(d) => d,
        _ => v.as_i64().unwrap_or(0) as f64,
    }
}

/// SNBT integer literal: sign, base, digits (separators removed), suffix `[u|s](b|s|i|l)`.
struct IntLit {
    neg: bool,
    radix: u32,
    digits: String,
    /// Forced signed (`s…`), forced unsigned (`u…`), otherwise according to the base.
    signed: Option<bool>,
    ty: Option<char>,
}

/// Run of digits of the base (separator `_` allowed inside only); returns the rest.
fn digit_run(s: &str, ok: impl Fn(char) -> bool) -> Result<(&str, &str), &'static str> {
    let end = s.find(|c: char| !(ok(c) || c == '_')).unwrap_or(s.len());
    let run = &s[..end];
    if run.is_empty() {
        return Err("snbt.parser.expected_decimal_numeral");
    }
    if run.starts_with('_') || run.ends_with('_') {
        return Err("snbt.parser.underscore_not_allowed");
    }
    Ok((run, &s[end..]))
}

fn split_sign(w: &str) -> (bool, &str) {
    match w.as_bytes().first() {
        Some(b'-') => (true, &w[1..]),
        Some(b'+') => (false, &w[1..]),
        _ => (false, w),
    }
}

/// Reading of an integer literal filling the whole word.
fn int_literal(w: &str) -> Result<IntLit, &'static str> {
    let (neg, body) = split_sign(w);
    let dec = |c: char| c.is_ascii_digit();
    let (radix, digits, rest) = if let Some(after0) = body.strip_prefix('0') {
        if let Some(h) = after0.strip_prefix(['x', 'X']) {
            let (d, r) = digit_run(h, |c| c.is_ascii_hexdigit()).map_err(|e| {
                if e == "snbt.parser.expected_decimal_numeral" { "snbt.parser.expected_hex_numeral" } else { e }
            })?;
            (16, d, r)
        } else if let Some(Ok((d, r))) = after0.strip_prefix(['b', 'B']).map(|b| digit_run(b, |c| c == '0' || c == '1'))
        {
            (2, d, r)
        } else if after0.starts_with(|c: char| c.is_ascii_digit() || c == '_') {
            digit_run(after0, dec)?;
            return Err("snbt.parser.leading_zero_not_allowed");
        } else {
            (10, "0", after0)
        }
    } else {
        let (d, r) = digit_run(body, dec)?;
        (10, d, r)
    };
    let mut cs = rest.chars();
    let kind = |c: Option<char>| c.map(|c| c.to_ascii_lowercase()).filter(|c| matches!(c, 'b' | 's' | 'i' | 'l'));
    let (signed, ty) = match (cs.next().map(|c| c.to_ascii_lowercase()), cs.clone().next()) {
        (None, _) => (None, None),
        (Some(p @ ('u' | 's')), Some(t)) if kind(Some(t)).is_some() => {
            cs.next();
            (Some(p == 's'), kind(Some(t)))
        }
        (Some(t), _) if kind(Some(t)).is_some() => (None, kind(Some(t))),
        _ => return Err("snbt.parser.invalid_unquoted_start"),
    };
    if cs.next().is_some() {
        return Err("snbt.parser.invalid_unquoted_start");
    }
    Ok(IntLit { neg, radix, digits: digits.replace('_', ""), signed, ty })
}

/// Value of an integer literal at the given type (`b`, `s`, `i`, `l`): signed bounds, or unsigned
/// (base 16 and 2 by default, or suffix `u`) then folded onto the width of the type.
fn int_value(l: &IntLit, ty: char) -> Result<Nbt, &'static str> {
    let signed = l.signed.unwrap_or(l.radix == 10);
    if !signed && l.neg {
        return Err("snbt.parser.expected_non_negative_number");
    }
    let bits = match ty {
        'b' => 8,
        's' => 16,
        'i' => 32,
        _ => 64,
    };
    let m = u128::from_str_radix(&l.digits, l.radix).map_err(|_| "snbt.parser.number_parse_failure")?;
    let v: i128 = if signed {
        let m = i128::try_from(m).map_err(|_| "snbt.parser.number_parse_failure")?;
        let v = if l.neg { -m } else { m };
        if v < -(1i128 << (bits - 1)) || v >= 1i128 << (bits - 1) {
            return Err("snbt.parser.number_parse_failure");
        }
        v
    } else {
        if m >= 1u128 << bits {
            return Err("snbt.parser.number_parse_failure");
        }
        m as i128
    };
    Ok(match ty {
        'b' => Nbt::Byte(v as i8),
        's' => Nbt::Short(v as i16),
        'i' => Nbt::Int(v as i32),
        _ => Nbt::Long(v as i64),
    })
}

/// Real literal filling the whole word: `1.`, `1.5`, `.5`, `1e3`, `2f`…; `None` if the word does not have
/// the form of a real.
fn float_literal(w: &str) -> Option<Result<Nbt, &'static str>> {
    let (neg, body) = split_sign(w);
    fn run(s: &str) -> Result<(&str, &str), &'static str> {
        digit_run(s, |c| c.is_ascii_digit())
    }
    let mut text = String::from(if neg { "-" } else { "" });
    let (rest, has_dot) = if let Some(frac) = body.strip_prefix('.') {
        let (f, r) = match run(frac) {
            Ok(x) => x,
            Err(_) => return None,
        };
        text.push('.');
        text.push_str(f);
        (r, true)
    } else {
        let (whole, r) = match run(body) {
            Ok(x) => x,
            Err(e) if e == "snbt.parser.underscore_not_allowed" => return Some(Err(e)),
            Err(_) => return None,
        };
        text.push_str(whole);
        match r.strip_prefix('.') {
            Some(after) => {
                text.push('.');
                match run(after) {
                    Ok((f, r2)) => {
                        text.push_str(f);
                        (r2, true)
                    }
                    Err(e) if e == "snbt.parser.underscore_not_allowed" => return Some(Err(e)),
                    Err(_) => (after, true),
                }
            }
            None => (r, false),
        }
    };
    let mut rest = rest;
    let mut has_exp = false;
    if let Some(e) = rest.strip_prefix(['e', 'E']) {
        let (eneg, e) = split_sign(e);
        if let Ok((d, r)) = run(e) {
            text.push('e');
            if eneg {
                text.push('-');
            }
            text.push_str(d);
            rest = r;
            has_exp = true;
        }
    }
    let suffix = match rest.chars().next().map(|c| c.to_ascii_lowercase()) {
        None => None,
        Some(c @ ('f' | 'd')) if rest.len() == 1 => Some(c),
        _ => return None,
    };
    if !has_dot && !has_exp && suffix.is_none() {
        return None;
    }
    let clean = text.replace('_', "");
    Some(if suffix == Some('f') {
        match clean.parse::<f32>() {
            Ok(v) if v.is_finite() => Ok(Nbt::Float(v)),
            Ok(_) => Err("snbt.parser.infinity_not_allowed"),
            Err(_) => Err("snbt.parser.number_parse_failure"),
        }
    } else {
        match clean.parse::<f64>() {
            Ok(v) if v.is_finite() => Ok(Nbt::Double(v)),
            Ok(_) => Err("snbt.parser.infinity_not_allowed"),
            Err(_) => Err("snbt.parser.number_parse_failure"),
        }
    })
}

/// SNBT number (real tried first, then integer); `None` if the word is not a number.
fn number(w: &str) -> Option<Result<Nbt, &'static str>> {
    if !w.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    if let Some(f) = float_literal(w) {
        return Some(f);
    }
    Some(int_literal(w).and_then(|l| int_value(&l, l.ty.unwrap_or('i'))))
}

/// Reading of a complete value in a text.
pub fn parse(s: &str) -> Result<Nbt, &'static str> {
    let chars: Vec<char> = s.chars().collect();
    let mut p = P { s: &chars, i: 0 };
    let v = p.value()?;
    p.skip();
    if p.i < chars.len() {
        return Err("argument.nbt.trailing");
    }
    Ok(v)
}

/// Reading in the middle of a text (position advanced).
pub fn parse_at(s: &[char], i: &mut usize) -> Result<Nbt, &'static str> {
    let mut p = P { s, i: *i };
    let v = p.value()?;
    *i = p.i;
    Ok(v)
}

/// Quotes and escapes of a string.
pub fn quote(s: &str) -> String {
    let mut out = String::from(" ");
    let mut q: Option<char> = None;
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' | '\'' => {
                let qq = *q.get_or_insert(if c == '"' { '\'' } else { '"' });
                if qq == c {
                    out.push('\\');
                }
                out.push(c);
            }
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    let q = q.unwrap_or('"');
    out.replace_range(0..1, &q.to_string());
    out.push(q);
    out
}

/// Key left bare by the colored rendering of `data get`.
fn simple_key(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// Key left bare by the compact SNBT text: `[A-Za-z._][A-Za-z0-9._+-]*`, except `true` and `false`.
fn unquoted_key(k: &str) -> bool {
    let mut cs = k.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '.' | '_'))
        && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        && !k.eq_ignore_ascii_case("true")
        && !k.eq_ignore_ascii_case("false")
}

/// Hash code of a Java string (UTF-16).
fn java_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
}

/// Iteration order of a Java hash table filled in the given order (capacity 16,
/// doubled beyond 3/4). Approximation: no tree in the buckets (>8 collisions).
pub fn java_order(keys: &[&str]) -> Vec<usize> {
    let mut cap = 16usize;
    while keys.len() > cap * 3 / 4 {
        cap *= 2;
    }
    let mut idx: Vec<usize> = (0..keys.len()).collect();
    let bucket = |k: &str| {
        let h = java_hash(k);
        let h = (h ^ ((h as u32) >> 16) as i32) as u32 as usize;
        h & (cap - 1)
    };
    idx.sort_by_key(|&i| (bucket(keys[i]), i));
    idx
}

fn ordered(f: &[(String, Nbt)]) -> Vec<&(String, Nbt)> {
    let keys: Vec<&str> = f.iter().map(|(k, _)| k.as_str()).collect();
    java_order(&keys).into_iter().map(|i| &f[i]).collect()
}

/// Compact SNBT text.
pub fn to_snbt(v: &Nbt) -> String {
    let mut s = String::new();
    write(v, &mut s);
    s
}

fn write(v: &Nbt, s: &mut String) {
    match v {
        Nbt::Byte(b) => s.push_str(&format!("{b}b")),
        Nbt::Short(b) => s.push_str(&format!("{b}s")),
        Nbt::Int(b) => s.push_str(&b.to_string()),
        Nbt::Long(b) => s.push_str(&format!("{b}L")),
        Nbt::Float(f) => s.push_str(&format!("{}f", super::text::jf(*f))),
        Nbt::Double(f) => s.push_str(&format!("{}d", super::text::jd(*f))),
        Nbt::String(t) => s.push_str(&quote(t)),
        Nbt::ByteArray(a) => {
            s.push_str("[B;");
            s.push_str(&a.iter().map(|b| format!("{}B", *b as i8)).collect::<Vec<_>>().join(","));
            s.push(']');
        }
        Nbt::IntArray(a) => {
            s.push_str("[I;");
            s.push_str(&a.iter().map(i32::to_string).collect::<Vec<_>>().join(","));
            s.push(']');
        }
        Nbt::LongArray(a) => {
            s.push_str("[L;");
            s.push_str(&a.iter().map(|b| format!("{b}L")).collect::<Vec<_>>().join(","));
            s.push(']');
        }
        Nbt::List(l) => {
            s.push('[');
            for (i, e) in l.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                write(e, s);
            }
            s.push(']');
        }
        Nbt::Compound(f) => {
            s.push('{');
            // Keys sorted in the order of Java strings (UTF-16 units).
            let mut keys: Vec<&(String, Nbt)> = f.iter().collect();
            keys.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
            for (i, (k, e)) in keys.into_iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                if unquoted_key(k) {
                    s.push_str(k);
                } else {
                    s.push_str(&quote(k));
                }
                s.push(':');
                write(e, s);
            }
            s.push('}');
        }
    }
}

/// Colored rendering (no indentation).
pub fn pretty(v: &Nbt) -> Component {
    let mut out = Vec::new();
    pretty_into(v, &mut out, 0, true);
    join(out)
}

/// Same rendering without colors (`nbt` content with `plain`).
pub fn pretty_plain(v: &Nbt) -> Component {
    let mut out = Vec::new();
    pretty_into(v, &mut out, 0, false);
    join(out)
}

/// Coloring color, only for the colored rendering.
fn paint(rich: bool, c: Component, color: u32) -> Component {
    if rich { colored(c, color) } else { c }
}

fn num(out: &mut Vec<Component>, n: String, suffix: Option<&str>, rich: bool) {
    out.push(paint(rich, lit(n), GOLD));
    if let Some(s) = suffix {
        out.push(paint(rich, lit(s), RED));
    }
}

fn pretty_into(v: &Nbt, out: &mut Vec<Component>, depth: usize, rich: bool) {
    match v {
        Nbt::Byte(b) => num(out, b.to_string(), Some("b"), rich),
        Nbt::Short(b) => num(out, b.to_string(), Some("s"), rich),
        Nbt::Int(b) => num(out, b.to_string(), None, rich),
        Nbt::Long(b) => num(out, b.to_string(), Some("L"), rich),
        Nbt::Float(f) => num(out, super::text::jf(*f), Some("f"), rich),
        Nbt::Double(f) => num(out, super::text::jd(*f), Some("d"), rich),
        Nbt::String(t) => {
            let q = quote(t);
            let mut cs = q.chars();
            let first = cs.next().unwrap_or('"');
            let inner: String = q.chars().skip(1).take(q.chars().count().saturating_sub(2)).collect();
            out.push(lit(first.to_string()));
            out.push(paint(rich, lit(inner), GREEN));
            out.push(lit(first.to_string()));
        }
        Nbt::ByteArray(_) | Nbt::IntArray(_) | Nbt::LongArray(_) => {
            let (p, vals, suf): (&str, Vec<String>, Option<&str>) = match v {
                Nbt::ByteArray(a) => ("B", a.iter().map(|b| (*b as i8).to_string()).collect(), Some("b")),
                Nbt::IntArray(a) => ("I", a.iter().map(i32::to_string).collect(), None),
                Nbt::LongArray(a) => ("L", a.iter().map(i64::to_string).collect(), Some("L")),
                _ => unreachable!(),
            };
            out.push(lit("["));
            out.push(paint(rich, lit(p), RED));
            out.push(lit(";"));
            let n = vals.len();
            for (i, x) in vals.into_iter().take(128).enumerate() {
                out.push(lit(" "));
                num(out, x, suf, rich);
                if i != n - 1 {
                    out.push(lit(","));
                }
            }
            if n > 128 {
                out.push(paint(rich, lit("<...>"), 0xAAAAAA));
            }
            out.push(lit("]"));
        }
        Nbt::List(l) => {
            if l.is_empty() {
                out.push(lit("["));
                out.push(lit("]"));
            } else if depth >= 64 {
                out.push(lit("["));
                out.push(paint(rich, lit("<...>"), 0xAAAAAA));
                out.push(lit("]"));
            } else {
                let numeric = |e: &Nbt| {
                    matches!(
                        e,
                        Nbt::Byte(_) | Nbt::Short(_) | Nbt::Int(_) | Nbt::Long(_) | Nbt::Float(_) | Nbt::Double(_)
                    )
                };
                let wrap = l.len() < 8 && l.iter().any(|e| !numeric(e));
                out.push(lit("["));
                let n = l.len();
                for (i, e) in l.iter().enumerate() {
                    if wrap && i >= 128 {
                        break;
                    }
                    if !wrap && i != 0 {
                        out.push(lit(","));
                        out.push(lit(" "));
                    }
                    pretty_into(e, out, depth + 1, rich);
                    if wrap && i != n - 1 {
                        out.push(lit(","));
                        out.push(lit(" "));
                    }
                }
                if wrap && n > 128 {
                    out.push(paint(rich, lit("<...>"), 0xAAAAAA));
                }
                out.push(lit("]"));
            }
        }
        Nbt::Compound(f) => {
            if f.is_empty() {
                out.push(lit("{"));
                out.push(lit("}"));
            } else if depth >= 64 {
                out.push(lit("{"));
                out.push(paint(rich, lit("<...>"), 0xAAAAAA));
                out.push(lit("}"));
            } else {
                out.push(lit("{"));
                let items = ordered(f);
                let n = items.len();
                for (i, (k, e)) in items.into_iter().enumerate() {
                    if simple_key(k) {
                        out.push(paint(rich, lit(k.clone()), AQUA));
                    } else {
                        let q = quote(k);
                        let first: String = q.chars().take(1).collect();
                        let inner: String = q.chars().skip(1).take(q.chars().count() - 2).collect();
                        let mut c = lit(first.clone());
                        c.extra = vec![paint(rich, lit(inner), AQUA), lit(first)];
                        out.push(c);
                    }
                    out.push(lit(":"));
                    out.push(lit(" "));
                    pretty_into(e, out, depth + 1, rich);
                    if i != n - 1 {
                        out.push(lit(","));
                        out.push(lit(" "));
                    }
                }
                out.push(lit("}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_values() {
        assert_eq!(parse("1b"), Ok(Nbt::Byte(1)));
        assert_eq!(parse("-3s"), Ok(Nbt::Short(-3)));
        assert_eq!(parse("12"), Ok(Nbt::Int(12)));
        assert_eq!(parse("1.5"), Ok(Nbt::Double(1.5)));
        assert_eq!(parse("2f"), Ok(Nbt::Float(2.0)));
        assert_eq!(parse("5L"), Ok(Nbt::Long(5)));
        assert_eq!(parse("0x1F"), Ok(Nbt::Int(31)));
        assert_eq!(parse("true"), Ok(Nbt::Byte(1)));
        assert_eq!(parse("abc"), Ok(Nbt::String("abc".into())));
        assert_eq!(parse("'a\"b'"), Ok(Nbt::String("a\"b".into())));
        assert_eq!(
            parse("{a:1,b:[I;1,2],c:{d:\"x\"}}"),
            Ok(Nbt::Compound(vec![
                ("a".into(), Nbt::Int(1)),
                ("b".into(), Nbt::IntArray(vec![1, 2])),
                ("c".into(), Nbt::Compound(vec![("d".into(), Nbt::String("x".into()))])),
            ]))
        );
        assert_eq!(parse("[1,2]"), Ok(Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)])));
        assert!(parse("{a:1").is_err());
        assert_eq!(parse("1 x"), Err("argument.nbt.trailing"));
    }

    /// Bounds and suffixes of numbers, like the game's grammar.
    #[test]
    fn numbers() {
        let err = |s: &str| parse(s).unwrap_err();
        assert_eq!(err("300b"), "snbt.parser.number_parse_failure");
        assert_eq!(err("70000s"), "snbt.parser.number_parse_failure");
        assert_eq!(err("2147483648"), "snbt.parser.number_parse_failure");
        assert_eq!(err("9223372036854775808L"), "snbt.parser.number_parse_failure");
        assert_eq!(parse("-9223372036854775808L"), Ok(Nbt::Long(i64::MIN)));
        assert_eq!(parse("200ub"), Ok(Nbt::Byte(-56)));
        assert_eq!(err("-1ub"), "snbt.parser.expected_non_negative_number");
        assert_eq!(parse("1sb"), Ok(Nbt::Byte(1)));
        assert_eq!(parse("0xFF"), Ok(Nbt::Int(255)));
        assert_eq!(parse("0xFFFFFFFF"), Ok(Nbt::Int(-1)));
        assert_eq!(parse("0x80000000"), Ok(Nbt::Int(i32::MIN)));
        assert_eq!(err("-0x1"), "snbt.parser.expected_non_negative_number");
        assert_eq!(parse("-0x10sb"), Ok(Nbt::Byte(-16)));
        assert_eq!(err("0xFFsb"), "snbt.parser.number_parse_failure");
        assert_eq!(parse("0xFFs"), Ok(Nbt::Short(255)));
        assert_eq!(parse("0x1b"), Ok(Nbt::Int(27)));
        assert_eq!(parse("0xFFub"), Ok(Nbt::Byte(-1)));
        assert_eq!(parse("0b"), Ok(Nbt::Byte(0)));
        assert_eq!(parse("0b101"), Ok(Nbt::Int(5)));
        assert_eq!(parse("0b101b"), Ok(Nbt::Byte(5)));
        assert_eq!(err("01"), "snbt.parser.leading_zero_not_allowed");
        assert_eq!(parse("1_000"), Ok(Nbt::Int(1000)));
        assert_eq!(err("1_"), "snbt.parser.underscore_not_allowed");
        assert_eq!(err("1e40f"), "snbt.parser.infinity_not_allowed");
        assert_eq!(err("1e400"), "snbt.parser.infinity_not_allowed");
        assert_eq!(parse("1."), Ok(Nbt::Double(1.0)));
        assert_eq!(parse(".5"), Ok(Nbt::Double(0.5)));
        assert_eq!(parse("-.5"), Ok(Nbt::Double(-0.5)));
        assert_eq!(parse("1e3"), Ok(Nbt::Double(1000.0)));
        assert_eq!(parse("01.5"), Ok(Nbt::Double(1.5)));
        assert_eq!(parse("2F"), Ok(Nbt::Float(2.0)));
        // Direct rounding to single float (no double rounding via double).
        assert_eq!(parse("0.1f"), Ok(Nbt::Float(0.1)));
        assert!(parse("1e").is_err());
        assert!(parse("1u").is_err());
    }

    /// Typed arrays, empty key, `\N{…}`, built-in operations, heterogeneous lists.
    #[test]
    fn grammar() {
        let err = |s: &str| parse(s).unwrap_err();
        assert_eq!(err("[B;300]"), "snbt.parser.number_parse_failure");
        assert_eq!(err("[B;1s]"), "snbt.parser.invalid_array_element_type");
        assert_eq!(err("[I;1L]"), "snbt.parser.invalid_array_element_type");
        assert_eq!(parse("[I;1b,2s,3]"), Ok(Nbt::IntArray(vec![1, 2, 3])));
        assert_eq!(parse("[L;1,2L,3b]"), Ok(Nbt::LongArray(vec![1, 2, 3])));
        assert_eq!(parse("[B;200ub,1]"), Ok(Nbt::ByteArray(vec![200, 1])));
        assert!(parse("[I;1.5]").is_err());
        assert_eq!(err("{\"\":1}"), "snbt.parser.empty_key");
        assert_eq!(parse("\"\\N{LATIN SMALL LETTER A}\""), Ok(Nbt::String("a".into())));
        assert_eq!(parse("'\\N{snowman}'"), Ok(Nbt::String("☃".into())));
        assert_eq!(err("\"\\N{NOT A NAME}\""), "snbt.parser.invalid_character_name");
        assert_eq!(err("\"\\x4\""), "snbt.parser.expected_hex_escape");
        assert_eq!(parse("bool(0.5)"), Ok(Nbt::Byte(1)));
        assert_eq!(parse("bool(0)"), Ok(Nbt::Byte(0)));
        assert_eq!(parse("bool(true)"), Ok(Nbt::Byte(1)));
        assert_eq!(err("bool(\"x\")"), "snbt.parser.expected_number_or_boolean");
        assert_eq!(err("bool(1,2)"), "snbt.parser.no_such_operation");
        assert_eq!(err("foo(1)"), "snbt.parser.no_such_operation");
        assert_eq!(parse("uuid(\"0-0-0-0-1\")"), Ok(Nbt::IntArray(vec![0, 0, 0, 1])));
        assert!(parse_uuid("00000000-0000-0000-0000-0000000000001").is_none());
        assert_eq!(parse_uuid("00000000000000001-0-0-0-1"), Some((1u128 << 96) | 1));
        // Heterogeneous list kept as is.
        let l = parse("[1,\"a\",{b:1}]").unwrap();
        assert_eq!(l, Nbt::List(vec![Nbt::Int(1), Nbt::String("a".into()), parse("{b:1}").unwrap()]));
        assert_eq!(to_snbt(&l), "[1,\"a\",{b:1}]");
    }

    #[test]
    fn write_keys() {
        let n = parse("{b:1,a:1,\"1x\":2,\"true\":3,Z:4,\"_k\":5,\"a b\":6}").unwrap();
        assert_eq!(to_snbt(&n), "{\"1x\":2,Z:4,_k:5,a:1,\"a b\":6,b:1,\"true\":3}");
        assert_eq!(to_snbt(&Nbt::String("\u{1f}".into())), "\"\\x1F\"");
    }

    #[test]
    fn write_snbt() {
        assert_eq!(to_snbt(&Nbt::String("a'b".into())), "\"a'b\"");
        assert_eq!(to_snbt(&Nbt::String("a\"b".into())), "'a\"b'");
        assert_eq!(to_snbt(&Nbt::Double(1.0)), "1.0d");
        assert_eq!(to_snbt(&Nbt::List(vec![Nbt::Float(0.5), Nbt::Float(2.0)])), "[0.5f,2.0f]");
        // Order of a Java hash table: "id" (3355) before "Count"? — buckets 3355&15=11, …
        let keys = ["count", "id"];
        let o = java_order(&keys);
        assert_eq!(o.len(), 2);
    }

    /// `\N{…}` accepts exactly the names Java accepts (taken from the game's Java 25 runtime).
    #[test]
    fn character_names_java() {
        // Without a proper name: "BLOCK HEX" form only, constructed names refused.
        assert_eq!(char_by_name("HANGUL SYLLABLE GA"), None);
        assert_eq!(char_by_name("CJK UNIFIED IDEOGRAPH-4E00"), None);
        assert_eq!(char_by_name("TANGUT IDEOGRAPH-17000"), None);
        assert_eq!(char_by_name("CJK UNIFIED IDEOGRAPHS 4E00"), Some('\u{4E00}'));
        assert_eq!(char_by_name("cjk unified ideographs 4e00"), Some('\u{4E00}'));
        assert_eq!(char_by_name("HANGUL SYLLABLES AC00"), Some('가'));
        assert_eq!(char_by_name("TANGUT 17000"), Some('\u{17000}'));
        assert_eq!(char_by_name("PRIVATE USE AREA E000"), Some('\u{E000}'));
        assert_eq!(char_by_name("CJK UNIFIED IDEOGRAPHS 04E00"), None);
        assert_eq!(char_by_name("CJK UNIFIED IDEOGRAPHS +4E00"), None);
        assert_eq!(char_by_name("BASIC LATIN 41"), None);
        // Listed one by one in the Unicode database: Java names them.
        assert_eq!(char_by_name("CJK COMPATIBILITY IDEOGRAPH-F900"), Some('\u{F900}'));
        assert_eq!(char_by_name("KHITAN SMALL SCRIPT CHARACTER-18B00"), Some('\u{18B00}'));
        assert_eq!(char_by_name("NUSHU CHARACTER-1B170"), Some('\u{1B170}'));
        assert_eq!(char_by_name("  latin small letter a  "), Some('a'));
        // Controls named by Java; Unicode 17 additions unknown to Java 25.
        assert_eq!(char_by_name("NULL"), Some('\0'));
        assert_eq!(char_by_name("escape"), Some('\u{1B}'));
        assert_eq!(char_by_name("SIDETIC LETTER N01"), None);
        assert_eq!(char_by_name("CJK UNIFIED IDEOGRAPH-323B0"), None);
        assert_eq!(parse("\"\\N{HANGUL SYLLABLE GA}\""), Err("snbt.parser.invalid_character_name"));
    }

    /// Full comparison with the official game's table (`target/g3f10/java-names.txt`, a list
    /// extracted from the official game, outside the repository): to run by hand after an `unicode_names2` bump.
    #[test]
    #[ignore = "Java table outside the repository"]
    fn character_names_full_table() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/g3f10/java-names.txt");
        let text = std::fs::read_to_string(p).expect("Java table missing");
        let mut java = std::collections::HashMap::new();
        for l in text.lines() {
            let (h, n) = l.split_once(' ').unwrap();
            java.insert(u32::from_str_radix(h, 16).unwrap(), n.to_owned());
        }
        let ok_chars = |n: &str| n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ' '));
        let mut bad = Vec::new();
        for (&cp, n) in &java {
            if ok_chars(n) && char_by_name(n).map(|c| c as u32) != Some(cp) {
                bad.push(format!("Java {cp:X} {n}"));
            }
        }
        for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
            if let Some(n) = unicode_names2::name(c).map(|n| n.to_string())
                && java.get(&(c as u32)) != Some(&n)
                && char_by_name(&n).is_some()
            {
                bad.push(format!("Rust {:X} {n}", c as u32));
            }
        }
        bad.sort();
        assert!(bad.is_empty(), "{} differences: {:?}", bad.len(), &bad[..bad.len().min(40)]);
    }
}
