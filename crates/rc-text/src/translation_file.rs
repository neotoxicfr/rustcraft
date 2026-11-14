//! Reading of a translation file the way the game reads it: the bytes decoded as UTF-8 with the
//! invalid ones replaced, the JSON read by a tolerant reader (a byte order mark, comments, single
//! quoted or bare strings, `=` or `=>` after a name, `;` between entries). Only what that reader
//! also refuses rejects the file: a root that is not an object, a value that is not a plain string,
//! number or boolean (`null` included), a broken structure, or text left after the object.

/// The file is refused (the game then keeps its previous translations).
struct Refused;

/// Entries of a translation file in file order (a repeated key appears twice: the later one wins
/// once inserted), each value as its text; `None` when the game refuses the file.
pub(crate) fn read_entries(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    let text = String::from_utf8_lossy(bytes);
    let mut reader = Reader { text: &text, pos: 0 };
    let mut entries = Vec::new();
    reader.document(&mut entries).ok()?;
    Some(entries)
}

struct Reader<'a> {
    text: &'a str,
    /// Byte offset of the next character.
    pos: usize,
}

impl Reader<'_> {
    fn document(&mut self, out: &mut Vec<(String, String)>) -> Result<(), Refused> {
        // The tolerant reader skips a byte order mark and the `)]}'` line some servers put first.
        if self.text.starts_with('\u{feff}') {
            self.pos = '\u{feff}'.len_utf8();
        }
        self.skip_blanks()?;
        if self.rest().starts_with(")]}'\n") {
            self.pos += 5;
            self.skip_blanks()?;
        }
        if self.peek() != Some(b'{') {
            return Err(Refused);
        }
        self.pos += 1;
        self.object_entries(out)?;
        self.skip_blanks()?;
        if self.pos == self.text.len() { Ok(()) } else { Err(Refused) }
    }

    /// Entries up to the closing brace (the opening one already read).
    fn object_entries(&mut self, out: &mut Vec<(String, String)>) -> Result<(), Refused> {
        self.skip_blanks()?;
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(());
        }
        loop {
            self.skip_blanks()?;
            let key = self.name()?;
            self.name_separator()?;
            self.skip_blanks()?;
            let value = self.value()?;
            out.push((key, value));
            self.skip_blanks()?;
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some(b',' | b';') => self.pos += 1,
                _ => return Err(Refused),
            }
        }
    }

    fn name(&mut self) -> Result<String, Refused> {
        match self.peek() {
            Some(quote @ (b'"' | b'\'')) => self.quoted(quote),
            _ => self.bare_literal().map(str::to_owned),
        }
    }

    /// `:`, `=` or `=>`.
    fn name_separator(&mut self) -> Result<(), Refused> {
        self.skip_blanks()?;
        match self.peek() {
            Some(b':') => self.pos += 1,
            Some(b'=') => {
                self.pos += 1;
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                }
            }
            _ => return Err(Refused),
        }
        Ok(())
    }

    /// A plain value as its text: a string, a number as written, `true` or `false`.
    fn value(&mut self) -> Result<String, Refused> {
        match self.peek() {
            Some(quote @ (b'"' | b'\'')) => self.quoted(quote),
            Some(b'{' | b'[') => Err(Refused),
            _ => {
                let literal = self.bare_literal()?;
                // Keywords match whatever the case; a null value is not a plain value.
                if literal.eq_ignore_ascii_case("null") {
                    Err(Refused)
                } else if literal.eq_ignore_ascii_case("true") || literal.eq_ignore_ascii_case("false") {
                    Ok(literal.to_ascii_lowercase())
                } else {
                    Ok(literal.to_owned())
                }
            }
        }
    }

    /// An unquoted word: up to a blank or a character with a meaning in JSON; never empty.
    fn bare_literal(&mut self) -> Result<&str, Refused> {
        let ends = |b: u8| {
            matches!(b, b'/' | b'\\' | b';' | b'#' | b'=' | b'{' | b'}' | b'[' | b']' | b':' | b',')
                || matches!(b, b' ' | b'\t' | b'\x0c' | b'\r' | b'\n')
        };
        let rest = self.rest();
        let len = rest.bytes().position(ends).unwrap_or(rest.len());
        if len == 0 {
            return Err(Refused);
        }
        let literal = &self.text[self.pos..self.pos + len];
        self.pos += len;
        Ok(literal)
    }

    /// A string between `quote`s (the opening one at the current position), escapes resolved.
    fn quoted(&mut self, quote: u8) -> Result<String, Refused> {
        self.pos += 1;
        let mut out = Utf16Text::default();
        loop {
            // The run up to the next quote or backslash is copied at once rather than character by
            // character (both are ASCII, so the cut never falls inside a character).
            let rest = self.rest();
            let run = rest.bytes().position(|b| b == quote || b == b'\\').ok_or(Refused)?;
            out.push_str(&rest[..run]);
            let closing = rest.as_bytes()[run] == quote;
            self.pos += run + 1;
            if closing {
                return Ok(out.finish());
            }
            self.escape(&mut out)?;
        }
    }

    /// The character after a backslash. An unknown escape stands for the character itself.
    fn escape(&mut self, out: &mut Utf16Text) -> Result<(), Refused> {
        let c = self.rest().chars().next().ok_or(Refused)?;
        self.pos += c.len_utf8();
        match c {
            'u' => {
                let hex = self.rest().get(..4).ok_or(Refused)?;
                let unit = u16::from_str_radix(hex, 16).map_err(|_| Refused)?;
                // `from_str_radix` takes a leading sign: the four characters must all be digits.
                if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(Refused);
                }
                self.pos += 4;
                out.push_unit(unit);
            }
            't' => out.push_char('\t'),
            'b' => out.push_char('\u{8}'),
            'n' => out.push_char('\n'),
            'r' => out.push_char('\r'),
            'f' => out.push_char('\u{c}'),
            other => out.push_char(other),
        }
        Ok(())
    }

    /// Skips blanks and comments (`//` and `#` to the end of the line, `/* */`); an unclosed
    /// comment refuses the file. A lone `/` is left for the caller, which refuses it.
    fn skip_blanks(&mut self) -> Result<(), Refused> {
        loop {
            let rest = self.rest();
            match rest.as_bytes() {
                [b' ' | b'\t' | b'\r' | b'\n', ..] => self.pos += 1,
                [b'#', ..] | [b'/', b'/', ..] => {
                    self.pos += rest.find(['\n', '\r']).unwrap_or(rest.len());
                }
                [b'/', b'*', ..] => {
                    let end = rest[2..].find("*/").ok_or(Refused)?;
                    self.pos += 2 + end + 2;
                }
                _ => return Ok(()),
            }
        }
    }

    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }
}

/// Text built from characters and UTF-16 escapes: a `\u` pair of surrogates makes one character,
/// a lone surrogate (a Java string may hold one) becomes the replacement character.
#[derive(Default)]
struct Utf16Text {
    text: String,
    high_surrogate: Option<u16>,
}

impl Utf16Text {
    fn push_unit(&mut self, unit: u16) {
        if let Some(high) = self.high_surrogate.take() {
            if let Some(Ok(c)) = char::decode_utf16([high, unit]).next().filter(|_| (0xdc00..0xe000).contains(&unit)) {
                self.text.push(c);
                return;
            }
            self.text.push(char::REPLACEMENT_CHARACTER);
        }
        match unit {
            0xd800..0xdc00 => self.high_surrogate = Some(unit),
            0xdc00..0xe000 => self.text.push(char::REPLACEMENT_CHARACTER),
            _ => self.text.push(char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER)),
        }
    }

    fn push_char(&mut self, c: char) {
        if self.high_surrogate.take().is_some() {
            self.text.push(char::REPLACEMENT_CHARACTER);
        }
        self.text.push(c);
    }

    fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.high_surrogate.take().is_some() {
            self.text.push(char::REPLACEMENT_CHARACTER);
        }
        self.text.push_str(s);
    }

    fn finish(mut self) -> String {
        if self.high_surrogate.is_some() {
            self.text.push(char::REPLACEMENT_CHARACTER);
        }
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::read_entries as entries;

    fn pairs(list: &[(&str, &str)]) -> Option<Vec<(String, String)>> {
        Some(list.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
    }

    /// Plain JSON: strings, numbers as written, booleans, escapes and surrogate pairs.
    #[test]
    fn plain_json_reads_every_plain_value() {
        let file = br#"{"a":"A","n":1.50,"e":1e5,"t":true,"esc":"\"q\" \\ \/ \u00e9 \ud83d\ude00 \n"}"#;
        assert_eq!(
            entries(file),
            pairs(&[("a", "A"), ("n", "1.50"), ("e", "1e5"), ("t", "true"), ("esc", "\"q\" \\ / é \u{1F600} \n")])
        );
        assert_eq!(entries(b"{}"), pairs(&[]));
        assert_eq!(entries(br#"{"k":"x","k":"y"}"#), pairs(&[("k", "x"), ("k", "y")]), "repeated key kept in order");
    }

    /// A file saved in another encoding: the invalid bytes become replacement characters, the
    /// file is kept (it refused the whole table).
    #[test]
    fn invalid_utf8_bytes_are_replaced() {
        assert_eq!(entries(b"{\"menu.quit\":\"Arr\xeat\"}"), pairs(&[("menu.quit", "Arr\u{FFFD}t")]));
    }

    /// What the game's tolerant reader accepts: a byte order mark, comments, single quotes, bare
    /// names and values, `=`/`=>` and `;`, keywords in any case, unknown escapes.
    #[test]
    fn tolerant_syntax_is_accepted() {
        let file = "\u{feff}// header\n{ # note\n 'single': 'it\\'s' /* block */ , bare = word; arrow => 12 ;\n \"T\": TRUE, \"x\": \"\\a\" }\n// end";
        assert_eq!(
            entries(file.as_bytes()),
            pairs(&[("single", "it's"), ("bare", "word"), ("arrow", "12"), ("T", "true"), ("x", "a")])
        );
        assert_eq!(entries(b")]}'\n{\"a\":\"b\"}"), pairs(&[("a", "b")]), "non-execute prefix");
        assert_eq!(entries(br#"{"lone":"\ud83d!"}"#), pairs(&[("lone", "\u{FFFD}!")]), "lone surrogate");
    }

    /// What the game's reader refuses too: not an object, a null or nested value, broken
    /// structure, text after the object, an unclosed comment or string, a bad unicode escape.
    #[test]
    fn broken_files_are_refused() {
        for (file, why) in [
            (&br#"["a"]"#[..], "not an object"),
            (b"", "empty"),
            (b"// only a comment", "no value"),
            (br#"{"a":null}"#, "null"),
            (br#"{"a":NULL}"#, "null in capitals"),
            (br#"{"a":["x"]}"#, "array value"),
            (br#"{"a":{"b":"c"}}"#, "object value"),
            (b"{", "unclosed object"),
            (br#"{"a":"b",}"#, "trailing comma"),
            (br#"{"a" "b"}"#, "no separator"),
            (br#"{"a":"b"} x"#, "text after the object"),
            (br#"{"a":"b"}{}"#, "second object"),
            (br#"{"a":"b" /* x}"#, "unclosed comment"),
            (br#"{"a":"b"#, "unclosed string"),
            (br#"{"a":"\u12g4"}"#, "bad unicode escape"),
            (br#"{"a":"\u+123"}"#, "signed unicode escape"),
            (br#"{"a":/}"#, "lone slash"),
        ] {
            assert_eq!(entries(file), None, "{why}");
        }
    }
}
