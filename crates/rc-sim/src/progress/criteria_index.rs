//! Criteria of a trigger grouped by the identifier they name, so that an event only tests the
//! criteria able to match it (`RUSTCRAFT_OPT_ADVINDEX`, `RUSTCRAFT_OPT_RECIPEINDEX`). Built with the
//! tree; an item tag is kept by name and asked at the event, so a `/reload` of the tags needs no
//! rebuild (a `/reload` of the advancements builds a new tree, hence a new index).

use hashbrown::HashMap;
use serde_json::Value;

use super::Adv;

#[cfg(test)]
#[path = "criteria_index_tests.rs"]
mod tests;

/// The trigger of the inventory changes, indexed by the item of its single item predicate.
pub(crate) const INVENTORY_CHANGED: &str = "minecraft:inventory_changed";

/// Triggers indexed by an identifier field of their conditions, and that field.
const BY_FIELD: [(&str, &str); 4] = [
    ("minecraft:recipe_unlocked", "recipes"),
    ("minecraft:recipe_crafted", "recipes"),
    ("minecraft:crafter_recipe_crafted", "recipes"),
    ("minecraft:player_generates_container_loot", "loot_tables"),
];

/// Positions of criteria in their trigger's list (`Tree::by_trigger`), grouped by what they name.
#[derive(Debug, Default)]
pub struct CriteriaIndex {
    /// Full identifier → positions, ascending.
    by_id: HashMap<String, Vec<u32>>,
    /// Item tag, in the full form the item predicate asks for → positions, ascending.
    by_tag: Vec<(String, Vec<u32>)>,
    /// Criteria tested for every event: no identifier narrows them, or they carry a `player`
    /// condition (evaluated before the criterion's own test, possibly with a random draw).
    always: Vec<u32>,
}

/// What a criterion's conditions name, as far as the index is concerned.
enum Key {
    /// Matches only an event about one of these full identifiers.
    Ids(Vec<String>),
    /// Matches only an item of this tag.
    Tag(String),
    /// Any event may match.
    Always,
    /// No event can match (the field is neither an identifier nor a list).
    Never,
}

/// The indexes of every indexed trigger present in `by_trigger`.
pub(crate) fn build_all(
    list: &[Adv],
    by_trigger: &HashMap<String, Vec<(usize, usize)>>,
) -> HashMap<String, CriteriaIndex> {
    let mut out = HashMap::new();
    let conds = |name: &str| by_trigger.get(name).map(|l| l.iter().map(|&(a, c)| &list[a].criteria[c].cond));
    if let Some(c) = conds(INVENTORY_CHANGED) {
        out.insert(INVENTORY_CHANGED.to_owned(), CriteriaIndex::build(c, item_key));
    }
    for (name, field) in BY_FIELD {
        if let Some(c) = conds(name) {
            out.insert(name.to_owned(), CriteriaIndex::build(c, |cond| field_key(cond, field)));
        }
    }
    out
}

/// Key of an `inventory_changed` criterion: the `items` of its only item predicate. Zero or
/// several item predicates, or one without a usable `items`, are tested for every change.
fn item_key(cond: &Value) -> Key {
    let Some([only]) = cond.get("items").and_then(Value::as_array).map(Vec::as_slice) else { return Key::Always };
    match only.get("items") {
        Some(Value::String(s)) => match s.strip_prefix('#') {
            Some(tag) => Key::Tag(crate::commands::args::id(tag)),
            None => Key::Ids(vec![crate::commands::args::id(s)]),
        },
        // A `#tag` inside a list is compared as an identifier: kept as such (never an item).
        Some(Value::Array(a)) => Key::Ids(a.iter().filter_map(Value::as_str).map(crate::commands::args::id).collect()),
        _ => Key::Always,
    }
}

/// Key of a criterion naming recipes or loot tables in `field`: an identifier or a list of them
/// (anything else never matches).
fn field_key(cond: &Value, field: &str) -> Key {
    match cond.get(field) {
        Some(Value::String(s)) => Key::Ids(vec![crate::commands::args::id(s)]),
        Some(Value::Array(a)) => Key::Ids(a.iter().filter_map(Value::as_str).map(crate::commands::args::id).collect()),
        _ => Key::Never,
    }
}

impl CriteriaIndex {
    fn build<'a>(conds: impl Iterator<Item = &'a Value>, key_of: impl Fn(&Value) -> Key) -> CriteriaIndex {
        let mut index = CriteriaIndex::default();
        for (pos, cond) in conds.enumerate() {
            let pos = pos as u32;
            let key = if cond.get("player").is_some() { Key::Always } else { key_of(cond) };
            match key {
                Key::Ids(ids) => {
                    for id in ids {
                        let bucket = index.by_id.entry(id).or_default();
                        // A list naming the same identifier twice keeps the criterion once.
                        if bucket.last() != Some(&pos) {
                            bucket.push(pos);
                        }
                    }
                }
                Key::Tag(tag) => match index.by_tag.iter_mut().find(|(t, _)| *t == tag) {
                    Some((_, bucket)) => bucket.push(pos),
                    None => index.by_tag.push((tag, vec![pos])),
                },
                Key::Always => index.always.push(pos),
                Key::Never => {}
            }
        }
        index
    }

    /// Positions that may match an event about `id` (`None`: an emptied slot, which only the
    /// criteria tested for every event can match), ascending, written to `out`. `has_tag(tag, id)`
    /// answers the tag buckets.
    pub(crate) fn candidates(&self, id: Option<&str>, has_tag: impl Fn(&str, &str) -> bool, out: &mut Vec<u32>) {
        out.clear();
        out.extend_from_slice(&self.always);
        if let Some(id) = id {
            if let Some(bucket) = self.by_id.get(id) {
                out.extend_from_slice(bucket);
            }
            for (tag, bucket) in &self.by_tag {
                if has_tag(tag, id) {
                    out.extend_from_slice(bucket);
                }
            }
        }
        // Back to the list order: the hits are awarded in the order of the full scan.
        out.sort_unstable();
        out.dedup();
    }

    /// Sizes of the groups (identifiers, tags, always tested), for the tests and the bench notes.
    #[cfg(test)]
    pub(crate) fn sizes(&self) -> (usize, usize, usize) {
        (self.by_id.len(), self.by_tag.len(), self.always.len())
    }
}
