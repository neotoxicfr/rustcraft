//! `filter` section of `pack.mcmeta`, common to resource packs (`rc-assets`) and to
//! data packs (`rc-sim`): a pack hides the files of the packs placed below it.

use serde_json::Value;

/// `filter` section of `pack.mcmeta`: patterns (namespace, path); an absent pattern accepts
/// everything, a present pattern accepts by partial search.
#[derive(Debug, Clone)]
pub struct Filter {
    patterns: Vec<(Option<fancy_regex::Regex>, Option<fancy_regex::Regex>)>,
}

impl Filter {
    /// The game tests the namespace and the path separately: the filter applies to a
    /// namespace if ONE pattern accepts it, and hides a path there if ONE pattern (perhaps another) accepts it.
    pub fn hides(&self, namespace: &str, path: &str) -> bool {
        // Search too costly (backtracking limit): no match.
        let hit = |r: &Option<fancy_regex::Regex>, s: &str| r.as_ref().is_none_or(|r| r.is_match(s).unwrap_or(false));
        self.patterns.iter().any(|p| hit(&p.0, namespace)) && self.patterns.iter().any(|p| hit(&p.1, path))
    }

    /// Pack path (`assets/<ns>/<path>` or `data/<ns>/<path>`) hidden; files outside
    /// a namespace never are.
    pub fn hides_file(&self, file: &str) -> bool {
        file.strip_prefix("assets/")
            .or_else(|| file.strip_prefix("data/"))
            .and_then(|r| r.split_once('/'))
            .is_some_and(|(ns, p)| self.hides(ns, p))
    }
}

/// Java pattern: `\Q…\E` (literal text, absent from the engine) replaced by its escaping.
pub(crate) fn java_pattern(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut rest = p;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.chars().next() {
            Some('Q') => {
                let quoted = &after[1..];
                let end = quoted.find("\\E").unwrap_or(quoted.len());
                out.push_str(&fancy_regex::escape(&quoted[..end]));
                rest = quoted.get(end + 2..).unwrap_or_default();
            }
            Some(c) => {
                out.push('\\');
                out.push(c);
                rest = &after[c.len_utf8()..];
            }
            None => {
                out.push('\\');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reads the `filter` section; absent or unreadable (invalid pattern, missing `block`): no
/// filter, like the game. Patterns of Java syntax (lookahead and lookbehind assertions, back
/// references, possessive quantifiers, `\Q…\E`); a pattern our engine refuses is reported.
pub fn filter(meta: &Value) -> Option<Filter> {
    let block = meta.get("filter")?.get("block")?.as_array()?;
    let pat = |o: &serde_json::Map<String, Value>, k: &str| -> Option<Option<fancy_regex::Regex>> {
        match o.get(k) {
            None => Some(None),
            Some(v) => {
                let src = v.as_str()?;
                match fancy_regex::Regex::new(&java_pattern(src)) {
                    Ok(r) => Some(Some(r)),
                    Err(e) => {
                        eprintln!("pack: filter pattern refused {src:?} ({e}), filter ignored");
                        None
                    }
                }
            }
        }
    };
    let patterns = block
        .iter()
        .map(|p| {
            let o = p.as_object()?;
            Some((pat(o, "namespace")?, pat(o, "path")?))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Filter { patterns })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_tests_namespace_and_path_separately() {
        let f = filter(&serde_json::json!({"filter":{"block":[
            {"namespace":"^a$","path":"^zz"}, {"namespace":"^b$","path":"^x"}]}}))
        .expect("filter");
        // Quirk of the game: the path of the 2nd pattern also applies to the namespace of the 1st.
        assert!(f.hides("a", "x/y"));
        assert!(f.hides("b", "zz"));
        assert!(!f.hides("c", "x"));
        assert!(!f.hides("a", "y"));
        assert!(f.hides_file("assets/a/x.png"));
        assert!(!f.hides_file("pack.png"));
        assert!(filter(&serde_json::json!({"filter":{}})).is_none());
    }

    #[test]
    fn filter_patterns_use_java_syntax() {
        // Negative lookahead assertion: everything but the interface.
        let f = filter(&serde_json::json!({"filter":{"block":[{"path":"^(?!textures/gui/)"}]}})).expect("filter");
        assert!(f.hides("minecraft", "textures/block/stone.png"));
        assert!(!f.hides("minecraft", "textures/gui/title.png"));
        // Back reference, possessive quantifier, literal text \Q…\E.
        let f = filter(&serde_json::json!({"filter":{"block":[{"path":"^(a)\\1/"}]}})).expect("filter");
        assert!(f.hides("x", "aa/b") && !f.hides("x", "ab/b"));
        let f = filter(&serde_json::json!({"filter":{"block":[{"path":"^b++c"}]}})).expect("filter");
        assert!(f.hides("x", "bbc"));
        let f = filter(&serde_json::json!({"filter":{"block":[{"path":"\\Qa.b\\E$"}]}})).expect("filter");
        assert!(f.hides("x", "da.b") && !f.hides("x", "daxb"));
        assert_eq!(java_pattern("x\\Q(*\\E\\d\\Qz"), "x\\(\\*\\dz");
    }
}
