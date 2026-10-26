//! Suggestions (§ 3): builder, merging, sorting, "substring" and "resource" filters.

use std::cmp::Ordering;

/// Tooltip of a suggestion.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Tooltip {
    /// Translation key.
    Key(String),
    /// Component received from the server (index in the response).
    Server(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Suggestion {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub tooltip: Option<Tooltip>,
}

impl Suggestion {
    /// Extended to a wider range by copying the typed text around it.
    fn expand(&self, input: &[char], start: usize, end: usize) -> Suggestion {
        if (start, end) == (self.start, self.end) {
            return self.clone();
        }
        let mut text = String::new();
        if start < self.start {
            text.extend(&input[start..self.start.min(input.len())]);
        }
        text.push_str(&self.text);
        if end > self.end {
            text.extend(&input[self.end.min(input.len())..end.min(input.len())]);
        }
        Suggestion { start, end, text, tooltip: self.tooltip.clone() }
    }

    /// Full text once the suggestion is applied to `input`.
    pub fn apply(&self, input: &[char]) -> String {
        let mut s: String = input[..self.start.min(input.len())].iter().collect();
        s.push_str(&self.text);
        if self.end < input.len() {
            s.extend(&input[self.end..]);
        }
        s
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Suggestions {
    pub start: usize,
    pub end: usize,
    pub list: Vec<Suggestion>,
}

/// The game's case-insensitive comparison (Java 25, which works by code points since
/// Java 16): code point by code point, each one brought down to the lowercase of its uppercase
/// (simple mappings, one character to a single one), then the shortest first. An
/// uppercase that is written with several characters (`ß` → `SS`) has no simple mapping and
/// keeps the character; the only multiple lowercase (`İ` → `i̇`) starts with its simple
/// mapping. Characters outside the base plane fold like the others.
pub fn cmp_ignore_case(a: &str, b: &str) -> Ordering {
    fn fold(c: char) -> char {
        if c.is_ascii() {
            return c.to_ascii_lowercase();
        }
        let mut up = c.to_uppercase();
        let c = match (up.next(), up.next()) {
            (Some(m), None) => m,
            _ => c,
        };
        c.to_lowercase().next().unwrap_or(c)
    }
    // Common prefix skipped byte by byte (same bytes, same characters), cut at a character start,
    // common to both strings since their bytes are equal there.
    let mut cut = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(cut) {
        cut -= 1;
    }
    a[cut..].chars().map(fold).cmp(b[cut..].chars().map(fold))
}

impl Suggestions {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Common range, extended suggestions, duplicates removed, case-insensitive sort.
    pub fn create(input: &[char], list: Vec<Suggestion>) -> Suggestions {
        if list.is_empty() {
            return Suggestions::default();
        }
        let start = list.iter().map(|s| s.start).min().unwrap_or(0);
        let end = list.iter().map(|s| s.end).max().unwrap_or(0);
        let mut out: Vec<Suggestion> = Vec::with_capacity(list.len());
        for s in &list {
            let e = s.expand(input, start, end);
            if !out.contains(&e) {
                out.push(e);
            }
        }
        out.sort_by(|a, b| cmp_ignore_case(&a.text, &b.text));
        Suggestions { start, end, list: out }
    }

    /// Merging of several sources: a single source is returned as is.
    pub fn merge(input: &[char], mut all: Vec<Suggestions>) -> Suggestions {
        match all.len() {
            0 => Suggestions::default(),
            1 => all.pop().unwrap_or_default(),
            _ => Suggestions::create(input, all.into_iter().flat_map(|s| s.list).collect()),
        }
    }
}

/// Builder on the input truncated at the cursor, from `start`.
pub struct Builder<'a> {
    pub input: &'a [char],
    pub start: usize,
    pub remaining: String,
    pub remaining_lower: String,
    pub out: Vec<Suggestion>,
}

impl<'a> Builder<'a> {
    pub fn new(input: &'a [char], start: usize) -> Self {
        let start = start.min(input.len());
        let remaining: String = input[start..].iter().collect();
        let remaining_lower = remaining.to_lowercase();
        Builder { input, start, remaining, remaining_lower, out: Vec::new() }
    }

    pub fn offset(&self, start: usize) -> Builder<'a> {
        Builder::new(self.input, start)
    }

    pub fn suggest(&mut self, text: impl Into<String>) {
        self.suggest_tip(text, None);
    }

    pub fn suggest_tip(&mut self, text: impl Into<String>, tooltip: Option<Tooltip>) {
        let text = text.into();
        if text == self.remaining {
            return;
        }
        self.out.push(Suggestion { start: self.start, end: self.input.len(), text, tooltip });
    }

    /// Number (no equality test with the input, like the game).
    pub fn suggest_int(&mut self, v: i64) {
        self.out.push(Suggestion { start: self.start, end: self.input.len(), text: v.to_string(), tooltip: None });
    }

    pub fn add(&mut self, other: Builder<'_>) {
        self.out.extend(other.out);
    }

    pub fn build(self) -> Suggestions {
        Suggestions::create(self.input, self.out)
    }

    /// Values filtered by the "substring" rule.
    pub fn suggest_matching<S: AsRef<str>>(&mut self, values: impl IntoIterator<Item = S>) {
        let p = self.remaining_lower.clone();
        for v in values {
            if matches_sub_str(&p, &v.as_ref().to_lowercase()) {
                self.suggest(v.as_ref());
            }
        }
    }

    /// Identifiers filtered by the "resource" rule (§ 3.4).
    pub fn suggest_resources<S: AsRef<str>>(&mut self, values: impl IntoIterator<Item = S>) {
        self.suggest_resources_with(values, "", "", |_| None);
    }

    /// With prefix (`#`, `!`, `!#`) and suffix; optional tooltip.
    pub fn suggest_resources_with<S: AsRef<str>>(
        &mut self,
        values: impl IntoIterator<Item = S>,
        prefix: &str,
        suffix: &str,
        tip: impl Fn(&str) -> Option<Tooltip>,
    ) {
        let contents = self.remaining_lower.clone();
        let stripped = if contents.is_empty() {
            Some(String::new())
        } else {
            let common = contents.chars().zip(prefix.chars()).take_while(|(a, b)| a == b).count();
            if prefix.is_empty() {
                Some(contents.clone())
            } else if common == 0 {
                None
            } else {
                Some(contents.chars().skip(common).collect())
            }
        };
        let Some(c) = stripped else { return };
        for v in values {
            let v = v.as_ref();
            if resource_matches(&c, v) {
                let t = tip(v);
                self.suggest_tip(format!("{prefix}{v}{suffix}"), t);
            }
        }
    }
}

/// "Resource" rule: with `:` on the full identifier, otherwise namespace or path.
pub fn resource_matches(contents: &str, id: &str) -> bool {
    if contents.contains(':') {
        matches_sub_str(contents, id)
    } else {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
        matches_sub_str(contents, ns) || matches_sub_str(contents, path)
    }
}

/// "Substring" rule: start of the value or of a segment after `.`, `_` or `/`.
pub fn matches_sub_str(pattern: &str, input: &str) -> bool {
    let mut index = 0;
    loop {
        if input[index..].starts_with(pattern) {
            return true;
        }
        match input[index..].find(['.', '_', '/']) {
            Some(i) => index += i + 1,
            None => return false,
        }
    }
}

/// Pending suggestion request to the server (§ 3.2): truncated input to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's case-insensitive order: expected values recorded on the Java 25 runtime of the
    /// official launcher (OpenJDK 25.0.1), which compares by code points since Java 16 (simple
    /// mappings).
    #[test]
    fn game_case_insensitive_order() {
        use Ordering::{Equal, Greater, Less};
        assert_eq!(cmp_ignore_case("alice", "Bob"), Less);
        assert_eq!(cmp_ignore_case("ABC", "abc"), Equal);
        assert_eq!(cmp_ignore_case("_x", "a"), Less);
        assert_eq!(cmp_ignore_case("ab", "AB_"), Less);
        // ß has no simple uppercase: compared as ß (0xDF) and not as "SS".
        assert_eq!(cmp_ignore_case("ß", "SS"), Greater);
        assert_eq!(cmp_ignore_case("ß", "ss"), Greater);
        // Final sigma and ordinary sigma are conflated; İ is conflated with i.
        assert_eq!(cmp_ignore_case("ΑΣ", "ας"), Equal);
        assert_eq!(cmp_ignore_case("İ", "i"), Equal);
        // Outside the base plane: Deseret uppercase and lowercase are conflated (Java 25: 0).
        assert_eq!(cmp_ignore_case("\u{10400}", "\u{10428}"), Equal);
        assert_eq!(cmp_ignore_case("a\u{10400}", "a\u{10428}"), Equal);
        // Code point order: U+1F600 comes after U+FF41 (Java 25: 1).
        assert_eq!(cmp_ignore_case("\u{1F600}", "\u{FF21}"), Greater);
    }

    #[test]
    fn substring_rule() {
        assert!(matches_sub_str("stone", "smooth_stone"));
        assert!(!matches_sub_str("ne", "stone"));
        assert!(matches_sub_str("", "x"));
        assert!(resource_matches("stone", "minecraft:stone"));
        assert!(resource_matches("mine", "minecraft:stone"));
        assert!(!resource_matches("minecraft:sto", "foo:stone"));
    }

    #[test]
    fn merge_and_sort() {
        let input: Vec<char> = "/tp a".chars().collect();
        let mut b = Builder::new(&input, 4);
        b.suggest("Bob");
        b.suggest("alice");
        b.suggest("a");
        let s = b.build();
        assert_eq!(s.list.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["alice", "Bob"]);
        // Wider range: the typed text is copied.
        let other = Suggestions {
            start: 1,
            end: 5,
            list: vec![Suggestion { start: 1, end: 5, text: "tp @s".into(), tooltip: None }],
        };
        let m = Suggestions::merge(&input, vec![s, other]);
        assert_eq!((m.start, m.end), (1, 5));
        assert!(m.list.iter().any(|s| s.text == "tp alice"));
    }
}
