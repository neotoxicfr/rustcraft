//! Minimal JSON reader that keeps the text of numbers: the f32 fields of the data are read
//! directly from decimal text into f32 (a pass through f64 would round twice).
//!
//! Compact tree (`RUSTCRAFT_OPT_COMPACT_JSON`, default): numbers and object keys are stored in
//! place ([`SmolStr`]: up to 23 bytes without a heap block), an object is one shared block of
//! members sorted by key, and strings without escapes are copied in one piece. The old reader (`=0`) allocated a string
//! per number and per key and a tree map per object; it is kept, its result converted to the same
//! tree, as the old path and as the reference of the equivalence tests.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use smol_str::SmolStr;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Text of the number as written.
    Num(SmolStr),
    Str(String),
    Arr(Vec<Json>),
    Obj(JsonObject),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Json::Num(n) => {
                n.parse().ok().or_else(|| n.parse::<f64>().ok().filter(|v| v.fract() == 0.0).map(|v| v as i32))
            }
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_num(&self) -> bool {
        matches!(self, Json::Num(_))
    }
}

/// Members of a JSON object, sorted by key, one per key: a key written twice keeps its last value,
/// as a map would. A sorted slice rather than a map: the objects of the data hold a handful of
/// members, and one exactly sized block per object replaces a tree node per object. The block is
/// shared, never modified: copying a tree costs a counter per object instead of a copy of
/// everything under it (the pack's parse cache hands out such copies).
#[derive(Clone, PartialEq, Default)]
pub struct JsonObject {
    members: Arc<[(SmolStr, Json)]>,
}

impl JsonObject {
    /// Members `first..` of a reader's stack, written in any order, taken off as one object.
    fn take_members(stack: &mut Vec<(SmolStr, Json)>, first: usize) -> Self {
        let written = &mut stack[first..];
        // Stable sort: the members of one key stay in written order, so the last one can win.
        written.sort_by(|a, b| a.0.cmp(&b.0));
        if written.windows(2).any(|w| w[0].0 == w[1].0) {
            let mut members = stack.split_off(first);
            keep_last_of_each_key(&mut members);
            return JsonObject { members: members.into() };
        }
        JsonObject { members: stack.drain(first..).collect() }
    }

    /// Object of members already sorted by key, each key once.
    fn from_sorted(members: Vec<(SmolStr, Json)>) -> Self {
        JsonObject { members: members.into() }
    }

    /// The same block of members as `other` (cache proofs).
    #[cfg(test)]
    pub(crate) fn shares_members_with(&self, other: &JsonObject) -> bool {
        Arc::ptr_eq(&self.members, &other.members)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        let i = self.members.binary_search_by(|(k, _)| k.as_str().cmp(key)).ok()?;
        Some(&self.members[i].1)
    }

    /// Members in key order (the order of the map the old reader built).
    pub fn iter(&self) -> Members<'_> {
        Members(self.members.iter())
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Printed as a map, exactly like the old reader's tree map: the debug digests of the data that
/// hold JSON do not move.
impl std::fmt::Debug for JsonObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Keeps, of the members of each key (sorted, in written order within a key), the last written.
fn keep_last_of_each_key(members: &mut Vec<(SmolStr, Json)>) {
    members.dedup_by(|later, kept| {
        let same_key = later.0 == kept.0;
        if same_key {
            std::mem::swap(&mut later.1, &mut kept.1);
        }
        same_key
    });
}

/// Iterator over the members of an object, in key order.
pub struct Members<'a>(std::slice::Iter<'a, (SmolStr, Json)>);

impl<'a> Iterator for Members<'a> {
    type Item = (&'a str, &'a Json);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(k, v)| (k.as_str(), v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a> IntoIterator for &'a JsonObject {
    type Item = (&'a str, &'a Json);
    type IntoIter = Members<'a>;

    fn into_iter(self) -> Members<'a> {
        self.iter()
    }
}

/// Parses `text` with the reader chosen by `RUSTCRAFT_OPT_COMPACT_JSON`.
pub fn parse(text: &str) -> Result<Json, String> {
    parse_mode(text, crate::opt::compact_json())
}

/// [`parse`] with the chosen reader: compact (new) or allocating (old). Same tree and same errors.
pub fn parse_mode(text: &str, compact: bool) -> Result<Json, String> {
    let mut c = Cursor { text, i: 0 };
    let v = if compact { CompactReader::new(&mut c).value()? } else { AllocatingReader { c: &mut c }.value()? };
    c.ws();
    if c.i != text.len() {
        return Err(format!("text after the value at byte {}", c.i));
    }
    Ok(v)
}

/// Reading position in the text, with what both readers share.
struct Cursor<'a> {
    text: &'a str,
    i: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.i).copied()
    }

    /// Number of bytes from the cursor on that `keep` accepts, counted on the remaining slice: no
    /// bounds check nor write of the position per byte (half of the biome report is indentation).
    fn run_length(&self, keep: impl Fn(u8) -> bool) -> usize {
        self.text.as_bytes()[self.i..].iter().take_while(|&&b| keep(b)).count()
    }

    fn ws(&mut self) {
        self.i += self.run_length(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'));
    }

    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("invalid JSON at byte {}: {what}", self.i))
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            self.err(&format!("'{}' expected", c as char))
        }
    }

    /// `true`, `false` or `null` at the cursor.
    fn keyword(&mut self) -> Option<Json> {
        let rest = &self.text.as_bytes()[self.i..];
        let (len, v) = if rest.starts_with(b"true") {
            (4, Json::Bool(true))
        } else if rest.starts_with(b"false") {
            (5, Json::Bool(false))
        } else if rest.starts_with(b"null") {
            (4, Json::Null)
        } else {
            return None;
        };
        self.i += len;
        Some(v)
    }

    /// Text of the number at the cursor, as written. Its bytes are ASCII, so the slice always
    /// falls on character boundaries.
    fn number_text(&mut self) -> &'a str {
        let start = self.i;
        self.i += self.run_length(|b| matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'));
        &self.text[start..self.i]
    }

    /// String at the cursor (opening quote), escapes decoded byte by byte.
    fn decode_string(&mut self) -> Result<String, String> {
        if self.peek() != Some(b'"') {
            return self.err("string expected");
        }
        self.i += 1;
        let s = self.text.as_bytes();
        let mut out = Vec::new();
        loop {
            match s.get(self.i) {
                None => return self.err("unterminated string"),
                Some(b'"') => {
                    self.i += 1;
                    return String::from_utf8(out).map_err(|e| e.to_string());
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = *s.get(self.i).ok_or("truncated escape")?;
                    self.i += 1;
                    match c {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = s.get(self.i..self.i + 4).ok_or("truncated escape")?;
                            let code = u32::from_str_radix(std::str::from_utf8(hex).map_err(|e| e.to_string())?, 16)
                                .map_err(|e| e.to_string())?;
                            self.i += 4;
                            let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                            let mut buf = [0; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                Some(&c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }
}

/// New reader: numbers and keys stored in place, strings copied in one piece, every array and
/// object built in one exactly sized block.
struct CompactReader<'c, 'a> {
    c: &'c mut Cursor<'a>,
    /// Items of the arrays being read, innermost last: an array takes its own off the end once
    /// closed, instead of growing a vector of its own (a few reallocations per array).
    items: Vec<Json>,
    /// Members of the objects being read, the same way.
    members: Vec<(SmolStr, Json)>,
}

impl<'c, 'a> CompactReader<'c, 'a> {
    fn new(c: &'c mut Cursor<'a>) -> Self {
        CompactReader { c, items: Vec::new(), members: Vec::new() }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.c.ws();
        match self.c.peek() {
            None => self.c.err("unexpected end"),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?.into_owned())),
            Some(b't' | b'f' | b'n') => match self.c.keyword() {
                Some(v) => Ok(v),
                None => self.c.err("value expected"),
            },
            Some(c) if c == b'-' || c.is_ascii_digit() => Ok(Json::Num(SmolStr::new(self.c.number_text()))),
            Some(_) => self.c.err("value expected"),
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.c.i += 1;
        self.c.ws();
        if self.c.peek() == Some(b']') {
            self.c.i += 1;
            return Ok(Json::Arr(Vec::new()));
        }
        let first = self.items.len();
        loop {
            let v = self.value()?;
            self.items.push(v);
            self.c.ws();
            match self.c.peek() {
                Some(b',') => self.c.i += 1,
                Some(b']') => {
                    self.c.i += 1;
                    return Ok(Json::Arr(self.items.drain(first..).collect()));
                }
                _ => return self.c.err("',' or ']' expected"),
            }
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.c.i += 1;
        self.c.ws();
        if self.c.peek() == Some(b'}') {
            self.c.i += 1;
            return Ok(Json::Obj(JsonObject::default()));
        }
        let first = self.members.len();
        loop {
            self.c.ws();
            let key = SmolStr::from(self.string()?);
            self.c.eat(b':')?;
            let v = self.value()?;
            self.members.push((key, v));
            self.c.ws();
            match self.c.peek() {
                Some(b',') => self.c.i += 1,
                Some(b'}') => {
                    self.c.i += 1;
                    return Ok(Json::Obj(JsonObject::take_members(&mut self.members, first)));
                }
                _ => return self.c.err("',' or '}' expected"),
            }
        }
    }

    /// String at the cursor: a slice of the text when it holds no escape (nearly always),
    /// decoded otherwise.
    fn string(&mut self) -> Result<Cow<'a, str>, String> {
        if self.c.peek() != Some(b'"') {
            return self.c.err("string expected");
        }
        let text: &'a str = self.c.text;
        let start = self.c.i + 1;
        let end = start + text.as_bytes()[start..].iter().take_while(|&&b| b != b'"' && b != b'\\').count();
        match text.as_bytes().get(end) {
            Some(b'"') => {
                self.c.i = end + 1;
                // Both ends next to a quote: always character boundaries.
                Ok(Cow::Borrowed(&text[start..end]))
            }
            Some(_) => self.c.decode_string().map(Cow::Owned),
            None => {
                self.c.i = end;
                self.c.err("unterminated string")
            }
        }
    }
}

/// Old reader: a string per number and per key, a tree map per object (then converted).
struct AllocatingReader<'c, 'a> {
    c: &'c mut Cursor<'a>,
}

impl AllocatingReader<'_, '_> {
    fn value(&mut self) -> Result<Json, String> {
        self.c.ws();
        match self.c.peek() {
            None => self.c.err("unexpected end"),
            Some(b'{') => {
                self.c.i += 1;
                let mut m = BTreeMap::new();
                self.c.ws();
                if self.c.peek() == Some(b'}') {
                    self.c.i += 1;
                    return Ok(Json::Obj(JsonObject::default()));
                }
                loop {
                    self.c.ws();
                    let k = self.c.decode_string()?;
                    self.c.eat(b':')?;
                    let v = self.value()?;
                    m.insert(k, v);
                    self.c.ws();
                    match self.c.peek() {
                        Some(b',') => self.c.i += 1,
                        Some(b'}') => {
                            self.c.i += 1;
                            let members = m.into_iter().map(|(k, v)| (SmolStr::from(k), v)).collect();
                            return Ok(Json::Obj(JsonObject::from_sorted(members)));
                        }
                        _ => return self.c.err("',' or '}' expected"),
                    }
                }
            }
            Some(b'[') => {
                self.c.i += 1;
                let mut a = Vec::new();
                self.c.ws();
                if self.c.peek() == Some(b']') {
                    self.c.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    self.c.ws();
                    match self.c.peek() {
                        Some(b',') => self.c.i += 1,
                        Some(b']') => {
                            self.c.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return self.c.err("',' or ']' expected"),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.c.decode_string()?)),
            Some(b't' | b'f' | b'n') => match self.c.keyword() {
                Some(v) => Ok(v),
                None => self.c.err("value expected"),
            },
            Some(c) if c == b'-' || c.is_ascii_digit() => Ok(Json::Num(SmolStr::from(self.c.number_text().to_owned()))),
            Some(_) => self.c.err("value expected"),
        }
    }
}

#[cfg(test)]
#[path = "json_tests.rs"]
mod tests;
