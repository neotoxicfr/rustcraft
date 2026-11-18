//! Command SNBT (§ 2.4.1): syntactic reading (values kept in reduced form).
//!
//! Whitespace is skipped before each symbol, unquoted name or number, like the
//! game's grammar. On failure, the error returned is that of the first dead end
//! met (the game keeps the furthest: identical in common cases).

use crate::reader::{Reader, is_unquoted, is_whitespace};
use crate::{CmdError, LITERAL_INCORRECT};

/// Value read (reduced form: only the distinctions useful to arguments).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Compound(Vec<(String, Value)>),
    List(Vec<Value>),
    Array,
    Str(String),
    Number,
    Float,
    Bool(bool),
}

const BUILTINS: [(&str, usize); 2] = [("bool", 1), ("uuid", 1)];

fn skip_ws(r: &mut Reader<'_>) {
    while r.can_read() && is_whitespace(r.peek()) {
        r.skip();
    }
}

fn expected(r: &Reader<'_>, what: &str) -> CmdError {
    r.err(LITERAL_INCORRECT, vec![what.to_owned()])
}

/// Expected symbol (whitespace skipped before).
fn sym(r: &mut Reader<'_>, c: char) -> Result<(), CmdError> {
    skip_ws(r);
    if r.next_is(c) {
        r.skip();
        Ok(())
    } else {
        Err(expected(r, &c.to_string()))
    }
}

fn try_sym(r: &mut Reader<'_>, c: char) -> bool {
    let save = r.cursor;
    skip_ws(r);
    if r.next_is(c) {
        r.skip();
        true
    } else {
        r.cursor = save;
        false
    }
}

fn can_start_number(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '+' | '-' | '.')
}

/// Full SNBT value (leading whitespace skipped).
pub fn value(r: &mut Reader<'_>) -> Result<Value, CmdError> {
    skip_ws(r);
    if !r.can_read() {
        return Err(r.err0("snbt.parser.expected_unquoted_string"));
    }
    match r.peek() {
        '"' | '\'' => Ok(Value::Str(quoted(r)?)),
        '{' => compound(r),
        '[' => list(r),
        c if can_start_number(c) => number(r),
        _ => unquoted_or_builtin(r),
    }
}

fn unquoted_word(r: &mut Reader<'_>) -> Result<String, CmdError> {
    skip_ws(r);
    let w = r.read_unquoted();
    if w.is_empty() {
        return Err(r.err0("snbt.parser.expected_unquoted_string"));
    }
    Ok(w)
}

fn unquoted_or_builtin(r: &mut Reader<'_>) -> Result<Value, CmdError> {
    let start = r.cursor;
    let w = unquoted_word(r)?;
    if w.chars().next().is_some_and(can_start_number) {
        r.cursor = start;
        return Err(r.err0("snbt.parser.invalid_unquoted_start"));
    }
    if try_sym(r, '(') {
        let mut args = Vec::new();
        loop {
            skip_ws(r);
            if r.next_is(')') {
                break;
            }
            args.push(value(r)?);
            if !try_sym(r, ',') {
                break;
            }
        }
        sym(r, ')')?;
        if !BUILTINS.contains(&(w.as_str(), args.len())) {
            return Err(r.err("snbt.parser.no_such_operation", vec![format!("{w}/{}", args.len())]));
        }
        return Ok(if w == "bool" { Value::Bool(true) } else { Value::Array });
    }
    if w.eq_ignore_ascii_case("true") {
        Ok(Value::Bool(true))
    } else if w.eq_ignore_ascii_case("false") {
        Ok(Value::Bool(false))
    } else {
        Ok(Value::Str(w))
    }
}

/// Quoted string with SNBT escapes.
pub fn quoted(r: &mut Reader<'_>) -> Result<String, CmdError> {
    skip_ws(r);
    let q = r.read();
    let mut out = String::new();
    loop {
        if !r.can_read() {
            return Err(expected(r, &q.to_string()));
        }
        let c = r.read();
        if c == q {
            return Ok(out);
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        if !r.can_read() {
            return Err(expected(r, "\\"));
        }
        let e = r.read();
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
                let start = r.cursor;
                for _ in 0..n {
                    if !r.can_read() || !r.peek().is_ascii_hexdigit() {
                        r.cursor = start;
                        return Err(r.err("snbt.parser.expected_hex_escape", vec![n.to_string()]));
                    }
                    r.skip();
                }
                let v = u32::from_str_radix(&r.slice(start, r.cursor), 16).unwrap_or(0);
                match char::from_u32(v) {
                    Some(ch) => out.push(ch),
                    None => {
                        r.cursor = start;
                        return Err(r.err("snbt.parser.invalid_codepoint", vec![format!("U+{v:08X}")]));
                    }
                }
            }
            'N' => {
                if !r.next_is('{') {
                    return Err(expected(r, "{"));
                }
                r.skip();
                let start = r.cursor;
                while r.can_read() && (r.peek().is_ascii_alphanumeric() || matches!(r.peek(), '-' | ' ')) {
                    r.skip();
                }
                if r.cursor == start {
                    return Err(r.err0("snbt.parser.invalid_character_name"));
                }
                if !r.next_is('}') {
                    return Err(expected(r, "}"));
                }
                r.skip();
                out.push('?');
            }
            _ => {
                r.cursor -= 1;
                return Err(expected(r, "b, s, t, n, f, r, \\, ', \", x, u, U, N"));
            }
        }
    }
}

fn compound(r: &mut Reader<'_>) -> Result<Value, CmdError> {
    sym(r, '{')?;
    let mut entries = Vec::new();
    loop {
        skip_ws(r);
        if r.next_is('}') {
            break;
        }
        let key_start = r.cursor;
        let key = if r.next_is('"') || r.next_is('\'') {
            quoted(r)?
        } else {
            let k = r.read_unquoted();
            if k.is_empty() {
                return Err(expected(r, "}"));
            }
            k
        };
        sym(r, ':')?;
        if key.is_empty() {
            r.cursor = key_start;
            return Err(r.err0("snbt.parser.empty_key"));
        }
        let v = value(r)?;
        entries.push((key, v));
        if !try_sym(r, ',') {
            break;
        }
    }
    sym(r, '}')?;
    Ok(Value::Compound(entries))
}

fn list(r: &mut Reader<'_>) -> Result<Value, CmdError> {
    sym(r, '[')?;
    // Typed array: `B;`, `I;`, `L;`.
    let save = r.cursor;
    skip_ws(r);
    if let (Some(t @ ('B' | 'I' | 'L')), Some(';')) = (r.peek_at(0), next_non_ws(r, 1)) {
        let _ = t;
        r.skip();
        sym(r, ';')?;
        loop {
            skip_ws(r);
            if r.next_is(']') {
                break;
            }
            let at = r.cursor;
            match number(r)? {
                Value::Number => {}
                _ => {
                    r.cursor = at;
                    return Err(r.err0("snbt.parser.invalid_array_element_type"));
                }
            }
            if !try_sym(r, ',') {
                break;
            }
        }
        sym(r, ']')?;
        return Ok(Value::Array);
    }
    r.cursor = save;
    let mut items = Vec::new();
    loop {
        skip_ws(r);
        if r.next_is(']') {
            break;
        }
        items.push(value(r)?);
        if !try_sym(r, ',') {
            break;
        }
    }
    sym(r, ']')?;
    Ok(Value::List(items))
}

fn next_non_ws(r: &Reader<'_>, mut off: usize) -> Option<char> {
    while let Some(c) = r.peek_at(off) {
        if !is_whitespace(c) {
            return Some(c);
        }
        off += 1;
    }
    None
}

/// Run of digits (base 2, 10 or 16) with inner `_`; empty → error.
fn numeral(r: &mut Reader<'_>, ok: fn(char) -> bool, empty: &'static str) -> Result<String, CmdError> {
    let start = r.cursor;
    while r.can_read() && (ok(r.peek()) || r.peek() == '_') {
        r.skip();
    }
    let s = r.slice(start, r.cursor);
    if s.is_empty() {
        return Err(r.err0(empty));
    }
    if s.starts_with('_') || s.ends_with('_') {
        return Err(r.err0("snbt.parser.underscore_not_allowed"));
    }
    Ok(s)
}

fn dec(c: char) -> bool {
    c.is_ascii_digit()
}

fn integer_suffix(r: &mut Reader<'_>) {
    let c = r.peek_at(0).map(|c| c.to_ascii_lowercase());
    let n = r.peek_at(1).map(|c| c.to_ascii_lowercase());
    match (c, n) {
        (Some('u' | 's'), Some('b' | 's' | 'i' | 'l')) => r.cursor += 2,
        (Some('b' | 's' | 'i' | 'l'), _) => r.cursor += 1,
        _ => {}
    }
}

fn number(r: &mut Reader<'_>) -> Result<Value, CmdError> {
    skip_ws(r);
    let start = r.cursor;
    if r.next_is('+') || r.next_is('-') {
        r.skip();
    }
    // Hexadecimal / binary / zero.
    if r.next_is('0') {
        let after = r.peek_at(1).map(|c| c.to_ascii_lowercase());
        if after == Some('x') {
            r.cursor += 2;
            numeral(r, |c| c.is_ascii_hexdigit(), "snbt.parser.expected_hex_numeral")?;
            integer_suffix(r);
            return Ok(Value::Number);
        }
        if after == Some('b') && r.peek_at(2).is_some_and(|c| c == '0' || c == '1') {
            r.cursor += 2;
            numeral(r, |c| c == '0' || c == '1', "snbt.parser.expected_binary_numeral")?;
            integer_suffix(r);
            return Ok(Value::Number);
        }
    }
    let whole_start = r.cursor;
    let whole = if r.can_read() && dec(r.peek()) {
        Some(numeral(r, dec, "snbt.parser.expected_decimal_numeral")?)
    } else {
        None
    };
    let mut is_float = false;
    if r.next_is('.') {
        r.skip();
        is_float = true;
        let has_frac = r.can_read() && dec(r.peek());
        if has_frac {
            numeral(r, dec, "snbt.parser.expected_decimal_numeral")?;
        } else if whole.is_none() {
            return Err(r.err0("snbt.parser.expected_decimal_numeral"));
        }
    } else if whole.is_none() {
        r.cursor = start;
        return Err(r.err0("snbt.parser.expected_decimal_numeral"));
    }
    if r.peek_at(0).is_some_and(|c| c == 'e' || c == 'E') {
        let save = r.cursor;
        r.skip();
        if r.next_is('+') || r.next_is('-') {
            r.skip();
        }
        if r.can_read() && dec(r.peek()) {
            numeral(r, dec, "snbt.parser.expected_decimal_numeral")?;
            is_float = true;
        } else {
            r.cursor = save;
        }
    }
    if r.peek_at(0).is_some_and(|c| matches!(c, 'f' | 'F' | 'd' | 'D')) {
        r.skip();
        return Ok(Value::Float);
    }
    if is_float {
        return Ok(Value::Float);
    }
    if let Some(w) = whole
        && w.len() > 1
        && w.starts_with('0')
    {
        r.cursor = whole_start;
        return Err(r.err0("snbt.parser.leading_zero_not_allowed"));
    }
    integer_suffix(r);
    Ok(Value::Number)
}

/// "As argument" value: the rest is left to the reader (separator checked elsewhere).
pub fn parse_argument(r: &mut Reader<'_>) -> Result<Value, CmdError> {
    value(r)
}

/// Compound required (error at the cursor after the value).
pub fn compound_argument(r: &mut Reader<'_>) -> Result<Value, CmdError> {
    let v = parse_argument(r)?;
    if !matches!(v, Value::Compound(_)) {
        return Err(r.err0("argument.nbt.expected.compound"));
    }
    Ok(v)
}

/// Character allowed in an unquoted key.
pub fn is_key_char(c: char) -> bool {
    is_unquoted(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> Value {
        let c: Vec<char> = s.chars().collect();
        let mut r = Reader::new(&c, 0);
        let v = value(&mut r).unwrap_or_else(|e| panic!("{s} : {e:?}"));
        assert_eq!(r.cursor, c.len(), "{s}");
        v
    }

    fn err(s: &str) -> &'static str {
        let c: Vec<char> = s.chars().collect();
        value(&mut Reader::new(&c, 0)).unwrap_err().key
    }

    #[test]
    fn value_forms() {
        assert!(
            matches!(ok("{a:1b, 'b c':\"x\\n\", d:[1,2,], e:[I;1,2], f:true}"), Value::Compound(v) if v.len() == 5)
        );
        ok("1.5e3f");
        ok("-.5");
        ok("0x1F_FFul");
        ok("0b101");
        ok("1_000");
        ok("bool(1)");
        ok("{ a : 1 }");
        assert_eq!(ok("TRUE"), Value::Bool(true));
        assert_eq!(err("01"), "snbt.parser.leading_zero_not_allowed");
        assert_eq!(err("1_"), "snbt.parser.underscore_not_allowed");
        assert_eq!(err("foo(1,2)"), "snbt.parser.no_such_operation");
        assert_eq!(err("{a:1"), LITERAL_INCORRECT);
        assert_eq!(err("[I;1.5]"), "snbt.parser.invalid_array_element_type");
    }
}
