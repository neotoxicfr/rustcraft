//! Input reader (`commands.md` § 2.3): cursor over characters, basic reads.

use crate::CmdError;

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    pub s: &'a [char],
    pub cursor: usize,
}

/// Whitespace in the game's sense (Unicode spaces, except non-breaking ones).
pub fn is_whitespace(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\u{A0}' | '\u{2007}' | '\u{202F}')
}

pub fn is_unquoted(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')
}

fn is_number(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == '-'
}

impl<'a> Reader<'a> {
    pub fn new(s: &'a [char], cursor: usize) -> Self {
        Reader { s, cursor }
    }

    pub fn can_read_n(&self, n: usize) -> bool {
        self.cursor + n <= self.s.len()
    }

    pub fn can_read(&self) -> bool {
        self.cursor < self.s.len()
    }

    pub fn peek(&self) -> char {
        self.s[self.cursor]
    }

    pub fn peek_at(&self, off: usize) -> Option<char> {
        self.s.get(self.cursor + off).copied()
    }

    /// Next character if there is one.
    pub fn next_is(&self, c: char) -> bool {
        self.s.get(self.cursor) == Some(&c)
    }

    pub fn skip(&mut self) {
        self.cursor += 1;
    }

    pub fn read(&mut self) -> char {
        let c = self.s[self.cursor];
        self.cursor += 1;
        c
    }

    pub fn remaining(&self) -> &'a [char] {
        &self.s[self.cursor.min(self.s.len())..]
    }

    pub fn slice(&self, a: usize, b: usize) -> String {
        self.s[a..b].iter().collect()
    }

    pub fn err(&self, key: &'static str, args: Vec<String>) -> CmdError {
        CmdError::new(key, args, self.cursor)
    }

    pub fn err0(&self, key: &'static str) -> CmdError {
        self.err(key, Vec::new())
    }

    pub fn skip_whitespace(&mut self) {
        while self.can_read() && is_whitespace(self.peek()) {
            self.skip();
        }
    }

    fn number_run(&mut self) -> String {
        let start = self.cursor;
        while self.can_read() && is_number(self.peek()) {
            self.skip();
        }
        self.slice(start, self.cursor)
    }

    fn number<T: std::str::FromStr>(&mut self, expected: &'static str, invalid: &'static str) -> Result<T, CmdError> {
        let start = self.cursor;
        let n = self.number_run();
        if n.is_empty() {
            return Err(self.err0(expected));
        }
        n.parse::<T>().map_err(|_| {
            self.cursor = start;
            self.err(invalid, vec![n])
        })
    }

    pub fn read_int(&mut self) -> Result<i32, CmdError> {
        self.number("parsing.int.expected", "parsing.int.invalid")
    }

    pub fn read_long(&mut self) -> Result<i64, CmdError> {
        self.number("parsing.long.expected", "parsing.long.invalid")
    }

    pub fn read_double(&mut self) -> Result<f64, CmdError> {
        self.number("parsing.double.expected", "parsing.double.invalid")
    }

    pub fn read_float(&mut self) -> Result<f32, CmdError> {
        self.number("parsing.float.expected", "parsing.float.invalid")
    }

    pub fn read_unquoted(&mut self) -> String {
        let start = self.cursor;
        while self.can_read() && is_unquoted(self.peek()) {
            self.skip();
        }
        self.slice(start, self.cursor)
    }

    pub fn read_quoted(&mut self) -> Result<String, CmdError> {
        if !self.can_read() {
            return Ok(String::new());
        }
        let q = self.peek();
        if q != '"' && q != '\'' {
            return Err(self.err0("parsing.quote.expected.start"));
        }
        self.skip();
        self.read_until(q)
    }

    pub fn read_until(&mut self, end: char) -> Result<String, CmdError> {
        let mut out = String::new();
        let mut escaped = false;
        while self.can_read() {
            let c = self.read();
            if escaped {
                if c != end && c != '\\' {
                    self.cursor -= 1;
                    return Err(self.err("parsing.quote.escape", vec![c.to_string()]));
                }
                out.push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == end {
                return Ok(out);
            } else {
                out.push(c);
            }
        }
        Err(self.err0("parsing.quote.expected.end"))
    }

    pub fn read_string(&mut self) -> Result<String, CmdError> {
        if !self.can_read() {
            return Ok(String::new());
        }
        let c = self.peek();
        if c == '"' || c == '\'' {
            self.skip();
            self.read_until(c)
        } else {
            Ok(self.read_unquoted())
        }
    }

    pub fn read_bool(&mut self) -> Result<bool, CmdError> {
        let start = self.cursor;
        let v = self.read_string()?;
        if v.is_empty() {
            return Err(self.err0("parsing.bool.expected"));
        }
        match v.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => {
                self.cursor = start;
                Err(self.err("parsing.bool.invalid", vec![v]))
            }
        }
    }

    pub fn expect(&mut self, c: char) -> Result<(), CmdError> {
        if self.next_is(c) {
            self.skip();
            Ok(())
        } else {
            Err(self.err("parsing.expected", vec![c.to_string()]))
        }
    }

    /// Up to the space (excluded).
    pub fn read_until_space(&mut self) -> String {
        let start = self.cursor;
        while self.can_read() && self.peek() != ' ' {
            self.skip();
        }
        self.slice(start, self.cursor)
    }
}

/// Character allowed in an identifier.
pub fn is_id_char(c: char) -> bool {
    c.is_ascii_digit() || c.is_ascii_lowercase() || matches!(c, '_' | ':' | '/' | '.' | '-')
}

fn valid_namespace(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || matches!(c, '_' | '.' | '-'))
}

/// Identifier (§ 2.4): greedy read, `minecraft:` by default, returned complete.
pub fn read_id(r: &mut Reader<'_>) -> Result<String, CmdError> {
    let start = r.cursor;
    while r.can_read() && is_id_char(r.peek()) {
        r.skip();
    }
    let s = r.slice(start, r.cursor);
    let (ns, path) = match s.split_once(':') {
        Some((ns, p)) => (if ns.is_empty() { "minecraft" } else { ns }, p),
        None => ("minecraft", s.as_str()),
    };
    if !valid_namespace(ns) || path.contains(':') {
        r.cursor = start;
        return Err(r.err0("argument.id.invalid"));
    }
    Ok(format!("{ns}:{path}"))
}

/// Splits like `String.split(" ")` (trailing empties removed).
pub fn java_split_space(s: &str) -> Vec<&str> {
    let mut v: Vec<&str> = s.split(' ').collect();
    while v.last() == Some(&"") {
        v.pop();
    }
    if s.is_empty() {
        v.push("");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn numbers_and_strings() {
        let s = r("12 -3.5 x");
        let mut rd = Reader::new(&s, 0);
        assert_eq!(rd.read_int(), Ok(12));
        rd.skip();
        assert_eq!(rd.read_double(), Ok(-3.5));
        rd.skip();
        assert_eq!(rd.read_int().unwrap_err().key, "parsing.int.expected");
        let s = r("1.5");
        assert_eq!(Reader::new(&s, 0).read_int().unwrap_err().key, "parsing.int.invalid");
        let s = r(r#""a\"b" 'c'"#);
        let mut rd = Reader::new(&s, 0);
        assert_eq!(rd.read_string().unwrap(), "a\"b");
        rd.skip();
        assert_eq!(rd.read_string().unwrap(), "c");
        let s = r(r#""a\x""#);
        assert_eq!(Reader::new(&s, 0).read_string().unwrap_err().key, "parsing.quote.escape");
    }

    #[test]
    fn parses_identifiers() {
        let s = r("stone foo:bar/baz Foo");
        let mut rd = Reader::new(&s, 0);
        assert_eq!(read_id(&mut rd).unwrap(), "minecraft:stone");
        rd.skip();
        assert_eq!(read_id(&mut rd).unwrap(), "foo:bar/baz");
        rd.skip();
        // Uppercase: empty read → empty valid identifier (empty path).
        assert_eq!(read_id(&mut rd).unwrap(), "minecraft:");
        let s = r("a:b:c");
        assert_eq!(read_id(&mut Reader::new(&s, 0)).unwrap_err().key, "argument.id.invalid");
    }

    #[test]
    fn java_split() {
        assert_eq!(java_split_space("~ "), vec!["~"]);
        assert_eq!(java_split_space("~ ~"), vec!["~", "~"]);
        assert_eq!(java_split_space(""), vec![""]);
    }
}
