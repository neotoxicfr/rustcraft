//! Text form of NBT for the tests (the game's `{key: value}` notation): enough to write the
//! cases readably and to print a difference.

use rc_proto::nbt::Nbt;

/// Parses `text`; panics on a malformed case (a test's own input).
pub(crate) fn nbt(text: &str) -> Nbt {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let value = value(&chars, &mut at);
    skip_space(&chars, &mut at);
    assert!(at == chars.len(), "trailing text at {at} in {text}");
    value
}

fn skip_space(c: &[char], at: &mut usize) {
    while *at < c.len() && c[*at].is_whitespace() {
        *at += 1;
    }
}

fn expect(c: &[char], at: &mut usize, ch: char) {
    skip_space(c, at);
    assert!(c.get(*at) == Some(&ch), "expected {ch} at {at}");
    *at += 1;
}

fn value(c: &[char], at: &mut usize) -> Nbt {
    skip_space(c, at);
    match c[*at] {
        '{' => {
            *at += 1;
            let mut fields = Vec::new();
            loop {
                skip_space(c, at);
                if c[*at] == '}' {
                    *at += 1;
                    return Nbt::Compound(fields);
                }
                let key = word(c, at);
                expect(c, at, ':');
                fields.push((key, value(c, at)));
                skip_space(c, at);
                if c[*at] == ',' {
                    *at += 1;
                }
            }
        }
        '[' => {
            *at += 1;
            let array = (c.get(*at + 1) == Some(&';')).then(|| c[*at]);
            if array.is_some() {
                *at += 2;
            }
            let mut items = Vec::new();
            loop {
                skip_space(c, at);
                if c[*at] == ']' {
                    *at += 1;
                    break;
                }
                items.push(value(c, at));
                skip_space(c, at);
                if c[*at] == ',' {
                    *at += 1;
                }
            }
            let ints = || items.iter().map(|v| v.as_i64().expect("array of integers"));
            match array {
                Some('B') => Nbt::ByteArray(ints().map(|v| v as u8).collect()),
                Some('I') => Nbt::IntArray(ints().map(|v| v as i32).collect()),
                Some('L') => Nbt::LongArray(ints().collect()),
                _ => Nbt::List(items),
            }
        }
        _ => {
            let quoted = matches!(c[*at], '"' | '\'');
            let text = word(c, at);
            if quoted { Nbt::String(text) } else { scalar(&text) }
        }
    }
}

/// A key or an unquoted / quoted word.
fn word(c: &[char], at: &mut usize) -> String {
    skip_space(c, at);
    let quote = c[*at];
    if quote == '"' || quote == '\'' {
        *at += 1;
        let mut s = String::new();
        while c[*at] != quote {
            if c[*at] == '\\' {
                *at += 1;
            }
            s.push(c[*at]);
            *at += 1;
        }
        *at += 1;
        return s;
    }
    let start = *at;
    // Unquoted words stop at `:` (keys); identifiers are written quoted.
    while *at < c.len() && (c[*at].is_alphanumeric() || "_-.+/".contains(c[*at])) {
        *at += 1;
    }
    c[start..*at].iter().collect()
}

/// A number with its type suffix (`1b`, `2s`, `3`, `4L`, `5.0f`, `6.0d` / `6.0`), `true` /
/// `false` as bytes, anything else as text.
fn scalar(text: &str) -> Nbt {
    let (body, suffix) = text.split_at(text.len() - text.chars().last().map_or(0, char::len_utf8));
    let parsed = match suffix {
        "b" | "B" => body.parse().ok().map(Nbt::Byte),
        "s" | "S" => body.parse().ok().map(Nbt::Short),
        "L" | "l" => body.parse().ok().map(Nbt::Long),
        "f" | "F" => body.parse().ok().map(Nbt::Float),
        "d" | "D" => body.parse().ok().map(Nbt::Double),
        _ => None,
    };
    parsed
        .or_else(|| text.parse().ok().map(Nbt::Int))
        .or_else(|| text.contains('.').then(|| text.parse().ok().map(Nbt::Double)).flatten())
        .unwrap_or_else(|| match text {
            "true" => Nbt::Byte(1),
            "false" => Nbt::Byte(0),
            _ => Nbt::String(text.to_owned()),
        })
}

/// `value` with every compound's keys sorted: the game keeps a compound's fields in no
/// particular order, so two conversions are compared on their content.
pub(crate) fn sorted(value: &Nbt) -> Nbt {
    match value {
        Nbt::Compound(fields) => {
            let mut fields: Vec<(String, Nbt)> = fields.iter().map(|(k, v)| (k.clone(), sorted(v))).collect();
            fields.sort_by(|a, b| a.0.cmp(&b.0));
            Nbt::Compound(fields)
        }
        Nbt::List(items) => Nbt::List(items.iter().map(sorted).collect()),
        v => v.clone(),
    }
}

/// One line of text for a value, to print a difference.
pub(crate) fn show(value: &Nbt) -> String {
    match value {
        Nbt::Byte(v) => format!("{v}b"),
        Nbt::Short(v) => format!("{v}s"),
        Nbt::Int(v) => format!("{v}"),
        Nbt::Long(v) => format!("{v}L"),
        Nbt::Float(v) => format!("{v:?}f"),
        Nbt::Double(v) => format!("{v:?}d"),
        Nbt::String(s) => format!("{s:?}"),
        Nbt::ByteArray(a) => format!("[B;{}]", a.iter().map(|b| (*b as i8).to_string()).collect::<Vec<_>>().join(",")),
        Nbt::IntArray(a) => format!("[I;{}]", a.iter().map(i32::to_string).collect::<Vec<_>>().join(",")),
        Nbt::LongArray(a) => format!("[L;{}]", a.iter().map(i64::to_string).collect::<Vec<_>>().join(",")),
        Nbt::List(items) => format!("[{}]", items.iter().map(show).collect::<Vec<_>>().join(",")),
        Nbt::Compound(fields) => {
            format!("{{{}}}", fields.iter().map(|(k, v)| format!("{k:?}:{}", show(v))).collect::<Vec<_>>().join(","))
        }
    }
}
