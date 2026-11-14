//! Adventure mode's block predicates: the `can_break` and `can_place_on` components of an item,
//! tested on the block a player in adventure mode breaks, primes or uses the item on (and on the
//! block an arrow hits). Each predicate tests the block set, the state properties (exact value or
//! bounds, in the game's own order of the values) and the block entity data (partial match); the
//! data components a predicate may also name are not tested on this path, like the game.
//!
//! The game keeps, in the component's value, the last block tested and its answer, and answers
//! again without testing for a block of the same state (and, when a data predicate accepted the
//! block, the same block entity data). An answer refused because of the data is therefore kept
//! for every block of that state until another state is tested: reproduced here, with the
//! component's content standing for the game's shared value.

use rc_proto::nbt::Nbt;

use super::P;
use crate::player::Stack;
use crate::server::Server;

/// The component an adventure predicate list comes from (each keeps its own memory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdventureComponent {
    CanBreak,
    CanPlaceOn,
}

impl AdventureComponent {
    fn key(self) -> &'static str {
        match self {
            AdventureComponent::CanBreak => "minecraft:can_break",
            AdventureComponent::CanPlaceOn => "minecraft:can_place_on",
        }
    }
}

/// The last block one predicate list was tested on, and its answer.
struct LastCheck {
    component: AdventureComponent,
    /// The component's value: two stacks carrying the same one share their memory.
    predicates: Nbt,
    state: u32,
    pos: P,
    /// The block entity data as it was then (without position), kept when a data predicate
    /// accepted the block: the memory then also requires the same data.
    data: Option<Nbt>,
    accepted: bool,
}

/// How many predicate lists keep their last block (the oldest is forgotten past it).
const REMEMBERED_LISTS: usize = 16;

/// Memory of the adventure predicates (one entry per predicate list recently tested).
#[derive(Default)]
pub(crate) struct AdventureMemory {
    last: Vec<LastCheck>,
}

/// Rank of a property value in the game's order: the number of an integer property, false
/// before true, the declaration order of the other values. `None`: not a value of the property.
fn value_rank(values: &[String], value: &str) -> Option<i64> {
    let is_bool = values.len() == 2 && values.iter().all(|v| v == "true" || v == "false");
    if is_bool {
        return match value {
            "false" => Some(0),
            "true" => Some(1),
            _ => None,
        };
    }
    if values.iter().all(|v| v.parse::<i32>().is_ok()) {
        // Read as a number (`05` is 5), within the property's values.
        let n = value.parse::<i32>().ok()?;
        return values.iter().any(|v| v.parse::<i32>() == Ok(n)).then_some(i64::from(n));
    }
    values.iter().position(|v| v == value).map(|i| i as i64)
}

/// One `state` entry of a predicate against the property's values and current index.
fn property_matches(values: &[String], current: usize, wanted: &Nbt) -> bool {
    let Some(have) = values.get(current).and_then(|v| value_rank(values, v)) else { return false };
    match wanted {
        Nbt::String(exact) => value_rank(values, exact) == Some(have),
        Nbt::Compound(_) => {
            let bound = |key: &str| wanted.get(key).map(|b| b.as_str().and_then(|b| value_rank(values, b)));
            let min_ok = bound("min").is_none_or(|m| m.is_some_and(|m| have >= m));
            let max_ok = bound("max").is_none_or(|m| m.is_some_and(|m| have <= m));
            min_ok && max_ok
        }
        _ => false,
    }
}

/// The predicates of a component's value: one predicate or a list of them (the former form,
/// a compound holding a `predicates` list, read too).
fn predicate_list(value: &Nbt) -> Vec<&Nbt> {
    match value {
        Nbt::List(l) => l.iter().collect(),
        Nbt::Compound(_) => match value.get("predicates") {
            Some(Nbt::List(l)) => l.iter().collect(),
            _ => vec![value],
        },
        _ => Vec::new(),
    }
}

/// The data a predicate's `nbt` field expects: a compound, or its text form.
fn expected_data(field: &Nbt) -> Option<Nbt> {
    match field {
        Nbt::Compound(_) => Some(field.clone()),
        Nbt::String(text) => crate::commands::snbt::parse(text).ok(),
        _ => None,
    }
}

impl Server {
    /// Adventure mode: the item carries a `can_break` of which a predicate accepts the block at `p`.
    pub(crate) fn adventure_can_break(&mut self, item: Option<&Stack>, p: P) -> bool {
        self.adventure_allows(item, p, AdventureComponent::CanBreak)
    }

    /// Adventure mode: like `adventure_can_break`, for the `can_place_on` component (use of an
    /// item on the clicked block).
    pub(crate) fn adventure_can_place_on(&mut self, item: Option<&Stack>, p: P) -> bool {
        self.adventure_allows(item, p, AdventureComponent::CanPlaceOn)
    }

    fn adventure_allows(&mut self, item: Option<&Stack>, p: P, component: AdventureComponent) -> bool {
        let Some(value) = item.and_then(|s| s.components.as_ref()).and_then(|c| c.get(component.key())) else {
            return false;
        };
        let state = self.get(p);
        if let Some(answer) = self.remembered_adventure_answer(component, value, state, p) {
            return answer;
        }
        let mut accepted_by_data = false;
        let accepted = predicate_list(value).into_iter().any(|pred| {
            let hit = self.adventure_predicate_matches(pred, state, p);
            accepted_by_data = hit && pred.get("nbt").is_some();
            hit
        });
        let data = if accepted_by_data { self.adventure_block_data(p) } else { None };
        let memory = &mut self.adventure.last;
        memory.retain(|m| !(m.component == component && m.predicates == *value));
        if memory.len() >= REMEMBERED_LISTS {
            memory.remove(0);
        }
        memory.push(LastCheck { component, predicates: value.clone(), state, pos: p, data, accepted });
        accepted
    }

    /// The answer kept for this predicate list, if the block at `p` is the same as the last one
    /// tested: same state, and when a data predicate accepted that one, block entities with the
    /// same data (the last one read as it is now while it is still in place).
    fn remembered_adventure_answer(&self, component: AdventureComponent, value: &Nbt, state: u32, p: P) -> Option<bool> {
        let last = self.adventure.last.iter().find(|m| m.component == component && m.predicates == *value)?;
        if last.state != state {
            return None;
        }
        let Some(kept) = &last.data else { return Some(last.accepted) };
        let then = self.adventure_block_data(last.pos);
        let now = self.adventure_block_data(p)?;
        (now == *then.as_ref().unwrap_or(kept)).then_some(last.accepted)
    }

    /// The block entity data at `p` as the memory compares it: id, data and components, without
    /// the position.
    fn adventure_block_data(&self, p: P) -> Option<Nbt> {
        let mut n = self.data_of_block(p)?;
        for k in ["x", "y", "z"] {
            crate::nbtio::remove(&mut n, k);
        }
        Some(n)
    }

    /// One predicate of an adventure component against the block at `p` of `state`: its blocks,
    /// its state properties, its block entity data (components are not tested on this path).
    fn adventure_predicate_matches(&self, pred: &Nbt, state: u32, p: P) -> bool {
        self.adventure_blocks_match(pred.get("blocks"), state)
            && self.adventure_state_matches(pred.get("state"), state)
            && pred.get("nbt").is_none_or(|field| {
                let full = self.data_of_block(p);
                expected_data(field).zip(full).is_some_and(|(want, have)| crate::commands::select::nbt_matches(&want, &have))
            })
    }

    /// The `blocks` field: absent (any block), a block name, a `#` tag, or a list of names.
    fn adventure_blocks_match(&self, blocks: Option<&Nbt>, state: u32) -> bool {
        let name = self.state_name(state);
        let id = u32::from(self.view().block(state));
        let hit = |k: &str| match k.strip_prefix('#') {
            Some(tag) => self
                .block_tags
                .get(tag)
                .or_else(|| rc_data::ids::with_key(&["minecraft:", tag], |k| self.block_tags.get(k)))
                .is_some_and(|t| t.binary_search(&id).is_ok()),
            None => k == name,
        };
        match blocks {
            None => true,
            Some(Nbt::String(k)) => hit(k),
            Some(Nbt::List(l)) => l.iter().any(|e| e.as_str().is_some_and(hit)),
            _ => false,
        }
    }

    /// The `state` field: each named property must exist on the block and match (a value, or
    /// `min` / `max` bounds).
    fn adventure_state_matches(&self, wanted: Option<&Nbt>, state: u32) -> bool {
        let Some(Nbt::Compound(props)) = wanted else { return wanted.is_none() };
        let v = self.view();
        props.iter().all(|(name, want)| {
            v.property_values_named(state, name).is_some_and(|(values, current)| property_matches(values, current, want))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Values compared in the game's order: numbers for integers (`05` is 5), false before true,
    /// declaration order for the others; a value the property does not have never matches.
    #[test]
    fn property_values_compare_in_the_game_order() {
        let age = values(&["0", "1", "2", "3"]);
        assert!(property_matches(&age, 2, &Nbt::String("02".into())));
        let range = |min: &str, max: &str| {
            Nbt::Compound(vec![("min".into(), Nbt::String(min.into())), ("max".into(), Nbt::String(max.into()))])
        };
        assert!(property_matches(&age, 2, &range("1", "2")));
        assert!(!property_matches(&age, 3, &range("1", "2")));
        assert!(!property_matches(&age, 1, &range("1", "9")), "bound outside the values: refused");
        let lit = values(&["true", "false"]);
        assert!(property_matches(&lit, 0, &Nbt::Compound(vec![("min".into(), Nbt::String("false".into()))])));
        assert!(!property_matches(&lit, 1, &Nbt::Compound(vec![("min".into(), Nbt::String("true".into()))])));
        let axis = values(&["x", "y", "z"]);
        assert!(property_matches(&axis, 1, &range("x", "y")));
        assert!(!property_matches(&axis, 2, &range("x", "y")));
        assert!(!property_matches(&axis, 1, &Nbt::String("Y".into())));
    }
}
