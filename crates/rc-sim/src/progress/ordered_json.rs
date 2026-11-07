//! A JSON object read with its top-level keys in their written order (`serde_json`'s map sorts
//! them, and its order-keeping feature would change the maps of the whole repository).

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;

/// Top-level entries of a JSON object, in the order of the text.
pub(crate) struct Entries(Vec<(String, Value)>);

impl Entries {
    /// A key written twice keeps its first place with its last value: what the sorted map of the
    /// former reading held, placed where the former text search found the key first.
    pub(crate) fn into_unique(self) -> Vec<(String, Value)> {
        let mut entries = self.0;
        let mut repeated: Vec<(usize, usize)> = Vec::new();
        {
            let mut first_place: hashbrown::HashMap<&str, usize> = hashbrown::HashMap::with_capacity(entries.len());
            for (at, (key, _)) in entries.iter().enumerate() {
                if let Some(&first) = first_place.get(key.as_str()) {
                    repeated.push((first, at));
                } else {
                    first_place.insert(key, at);
                }
            }
        }
        if repeated.is_empty() {
            return entries;
        }
        let mut keep = vec![true; entries.len()];
        for &(first, later) in &repeated {
            entries[first].1 = std::mem::take(&mut entries[later].1);
            keep[later] = false;
        }
        let mut kept = keep.into_iter();
        entries.retain(|_| kept.next().unwrap_or(false));
        entries
    }
}

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Entries, D::Error> {
        struct InOrder;
        impl<'de> Visitor<'de> for InOrder {
            type Value = Entries;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entries, A::Error> {
                let mut out = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(entry) = map.next_entry::<String, Value>()? {
                    out.push(entry);
                }
                Ok(Entries(out))
            }
        }
        d.deserialize_map(InOrder)
    }
}
