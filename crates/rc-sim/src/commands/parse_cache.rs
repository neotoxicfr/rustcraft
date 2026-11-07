//! Bounded cache text → parse for the commands run by text again and again (repeating command
//! blocks, minecarts, the player's repeated commands). A parse depends only on the permission
//! level, the text and the pack data (tree, registries, tags), never on teams, players or the
//! world: the cache is emptied when the packs reload, nothing else invalidates it.

use std::sync::Arc;

use hashbrown::HashMap;
use rc_text::Component;

use super::Parsed;

/// A cached parse: the command, or the error lines shown to the source.
pub type CachedParse = Result<Arc<Parsed>, Arc<Vec<Component>>>;

/// Texts kept per generation; two generations (recent, older) bound the cache to twice this.
const GENERATION: usize = 2048;

/// Permission levels 0 to 4.
const LEVELS: usize = 5;

/// Two generations of entries: a text found in the older one moves to the recent one; when the
/// recent one is full it becomes the older one and the former older one is dropped. Texts still
/// in use survive, the rest goes within two generations (an approximate least recently used,
/// without a list to maintain on every hit).
#[derive(Default)]
pub struct ParseCache {
    recent: [HashMap<String, CachedParse>; LEVELS],
    older: [HashMap<String, CachedParse>; LEVELS],
    recent_len: usize,
}

impl ParseCache {
    pub fn get(&mut self, op: u8, text: &str) -> Option<CachedParse> {
        let level = usize::from(op).min(LEVELS - 1);
        if let Some(hit) = self.recent[level].get(text) {
            return Some(hit.clone());
        }
        let (key, hit) = self.older[level].remove_entry(text)?;
        self.insert_owned(level, key, hit.clone());
        Some(hit)
    }

    pub fn insert(&mut self, op: u8, text: &str, parse: CachedParse) {
        self.insert_owned(usize::from(op).min(LEVELS - 1), text.to_owned(), parse);
    }

    fn insert_owned(&mut self, level: usize, text: String, parse: CachedParse) {
        if self.recent_len >= GENERATION {
            self.older = std::mem::take(&mut self.recent);
            self.recent_len = 0;
        }
        if self.recent[level].insert(text, parse).is_none() {
            self.recent_len += 1;
        }
    }

    pub fn clear(&mut self) {
        *self = ParseCache::default();
    }

    /// Texts kept (both generations).
    pub fn len(&self) -> usize {
        self.recent.iter().chain(&self.older).map(HashMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(n: usize) -> CachedParse {
        Err(Arc::new(vec![Component::text(n.to_string())]))
    }

    fn text_of(c: &CachedParse) -> String {
        format!("{:?}", c.as_ref().err().map(|v| &v[0]))
    }

    /// Bounded, per level, a text in use survives the generations, a forgotten one goes.
    #[test]
    fn keeps_recent_texts_and_stays_bounded() {
        let mut c = ParseCache::default();
        c.insert(2, "kept", parse(0));
        c.insert(4, "kept", parse(1));
        for n in 0..GENERATION * 5 {
            c.insert(2, &format!("cmd {n}"), parse(n));
            // `kept` is used every tick: always found.
            assert!(c.get(2, "kept").is_some(), "lost after {n} insertions");
            assert!(c.len() <= 2 * GENERATION + 2, "cache grew to {}", c.len());
        }
        assert_eq!(text_of(&c.get(2, "kept").expect("kept")), text_of(&parse(0)));
        assert!(c.get(4, "kept").is_none(), "an unused entry outlived two generations");
        assert!(c.get(2, "cmd 0").is_none());
        c.clear();
        assert!(c.is_empty());
    }
}
