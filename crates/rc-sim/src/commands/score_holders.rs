//! Score holders of the scoreboard: each holder's scores (objective → score), the game's listing
//! order of the holders, and the holders named by an entity UUID.
//!
//! `RUSTCRAFT_OPT_SCOREHOLDERMAP` (default 1) keeps the holders in a hash table: one lookup per
//! access, and the save and the start of tracking walk the holders once instead of searching
//! each one in a list (O(H) instead of O(H²)). `0`: the former list searched linearly. The order
//! a player or a save can see never comes from the table: it is the game's hash table, replayed
//! by [`HashOrder`].

use std::collections::BTreeMap;

use hashbrown::{HashMap, HashSet};

use super::hash_order::HashOrder;
use super::scores::Score;
use crate::nbtio::canonical_uuid;

/// One holder's scores, by objective name (the order of `players list <holder>` and of the save).
pub type HolderScores = BTreeMap<String, Score>;

#[derive(Debug, Clone)]
enum Table {
    /// Former layout (`RUSTCRAFT_OPT_SCOREHOLDERMAP=0`): searched linearly.
    List(Vec<(String, HolderScores)>),
    Map(HashMap<String, HolderScores>),
}

/// What a reset did to a holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reset {
    /// The holder has no score: nothing happened.
    UnknownHolder,
    /// Its last score went: the holder is forgotten.
    HolderForgotten,
    /// The named score went, others remain.
    ScoreRemoved,
    /// No such score: nothing removed.
    NothingRemoved,
}

#[derive(Debug, Clone)]
pub struct Holders {
    table: Table,
    /// Listing order of the game (its hash table replayed): holders only, no scores.
    order: HashOrder,
    /// Holders whose name is an entity UUID in canonical text (`RUSTCRAFT_OPT_SCOREUUIDSET`):
    /// an entity removal looks its UUID up here before formatting it. `None` when off.
    uuids: Option<HashSet<u128>>,
}

impl Holders {
    pub fn new(hashed: bool, track_uuids: bool) -> Holders {
        Holders {
            table: if hashed { Table::Map(HashMap::new()) } else { Table::List(Vec::new()) },
            order: HashOrder::new(16, 0.5),
            uuids: track_uuids.then(HashSet::new),
        }
    }

    pub fn is_empty(&self) -> bool {
        match &self.table {
            Table::List(l) => l.is_empty(),
            Table::Map(m) => m.is_empty(),
        }
    }

    pub fn get(&self, holder: &str) -> Option<&HolderScores> {
        match &self.table {
            Table::List(l) => l.iter().find(|(h, _)| h == holder).map(|(_, m)| m),
            Table::Map(m) => m.get(holder),
        }
    }

    /// The holder's scores, created empty (and listed) when it has none.
    pub fn entry(&mut self, holder: &str) -> &mut HolderScores {
        let (scores, created) = match &mut self.table {
            Table::List(l) => match l.iter().position(|(h, _)| h == holder) {
                Some(i) => (&mut l[i].1, false),
                None => {
                    l.push((holder.to_owned(), BTreeMap::new()));
                    (&mut l.last_mut().expect("just pushed").1, true)
                }
            },
            Table::Map(m) => match m.entry_ref(holder) {
                hashbrown::hash_map::EntryRef::Occupied(e) => (e.into_mut(), false),
                hashbrown::hash_map::EntryRef::Vacant(e) => (e.insert(BTreeMap::new()), true),
            },
        };
        if created {
            self.order.insert(holder);
            if let (Some(uuids), Some(u)) = (&mut self.uuids, canonical_uuid(holder)) {
                uuids.insert(u);
            }
        }
        scores
    }

    /// `holder`'s score of `objective`, created (default) if needed; `true` if this access
    /// created it. The objective name is copied only when the score is created.
    pub fn open(&mut self, holder: &str, objective: &str) -> (&mut Score, bool) {
        let scores = self.entry(holder);
        // Two lookups when the score exists: the borrow checker refuses returning the first
        // `get_mut` and inserting afterwards, and `entry` would copy the name on every access.
        if scores.contains_key(objective) {
            (scores.get_mut(objective).expect("present"), false)
        } else {
            (scores.entry(objective.to_owned()).or_default(), true)
        }
    }

    /// Removes one score of `holder` (`Some`) or all of them (`None`); a holder left without any
    /// score is forgotten.
    pub fn reset(&mut self, holder: &str, objective: Option<&str>) -> Reset {
        // A holder left with scores returns at once; an emptied one leaves the table here and
        // is forgotten below.
        match &mut self.table {
            Table::List(l) => {
                let Some(i) = l.iter().position(|(h, _)| h == holder) else { return Reset::UnknownHolder };
                let removed = remove_scores(&mut l[i].1, objective);
                if !l[i].1.is_empty() {
                    return removed;
                }
                l.remove(i);
            }
            Table::Map(m) => {
                let Some(scores) = m.get_mut(holder) else { return Reset::UnknownHolder };
                let removed = remove_scores(scores, objective);
                if !scores.is_empty() {
                    return removed;
                }
                m.remove(holder);
            }
        }
        self.order.remove(holder);
        if let (Some(uuids), Some(u)) = (&mut self.uuids, canonical_uuid(holder)) {
            uuids.remove(&u);
        }
        Reset::HolderForgotten
    }

    /// Scores of a removed objective, erased from every holder (holders stay listed).
    pub fn remove_objective(&mut self, objective: &str) {
        match &mut self.table {
            Table::List(l) => l.iter_mut().for_each(|(_, m)| {
                m.remove(objective);
            }),
            Table::Map(m) => m.values_mut().for_each(|m| {
                m.remove(objective);
            }),
        }
    }

    /// Could `uuid` name a holder? `None` when the UUID holders are not tracked.
    pub fn names_uuid(&self, uuid: u128) -> Option<bool> {
        self.uuids.as_ref().map(|s| s.contains(&uuid))
    }

    /// Holders in the order of a direct walk of the game's table.
    pub fn listed(&self) -> impl Iterator<Item = &str> {
        self.order.iter()
    }

    /// Holders in the order of a stream walk of the game's table (the save).
    pub fn streamed(&self) -> impl Iterator<Item = &str> {
        self.order.iter_stream()
    }

    /// [`Holders::listed`] with each holder's scores (every listed holder has an entry:
    /// [`Holders::check_consistent`]).
    pub fn listed_scores(&self) -> impl Iterator<Item = (&str, &HolderScores)> {
        self.listed().filter_map(|h| Some((h, self.get(h)?)))
    }

    /// [`Holders::streamed`] with each holder's scores.
    pub fn streamed_scores(&self) -> impl Iterator<Item = (&str, &HolderScores)> {
        self.streamed().filter_map(|h| Some((h, self.get(h)?)))
    }

    /// Listing order, table and UUID set hold the same holders.
    #[cfg(test)]
    pub fn check_consistent(&self) {
        let listed: std::collections::BTreeSet<&str> = self.listed().collect();
        let stored: std::collections::BTreeSet<&str> = match &self.table {
            Table::List(l) => l.iter().map(|(h, _)| h.as_str()).collect(),
            Table::Map(m) => m.keys().map(String::as_str).collect(),
        };
        assert_eq!(listed, stored, "listing order and table differ");
        assert_eq!(listed.len(), self.listed().count(), "a holder listed twice");
        if let Some(uuids) = &self.uuids {
            let expected: HashSet<u128> = stored.iter().filter_map(|h| canonical_uuid(h)).collect();
            assert_eq!(*uuids, expected, "UUID holders differ from the table");
        }
    }
}

/// What removing one score (`Some`) or all of them (`None`) did, as long as the holder keeps
/// other scores (an emptied holder is forgotten).
fn remove_scores(scores: &mut HolderScores, objective: Option<&str>) -> Reset {
    match objective {
        Some(o) if scores.remove(o).is_some() => Reset::ScoreRemoved,
        Some(_) => Reset::NothingRemoved,
        None => {
            scores.clear();
            Reset::HolderForgotten
        }
    }
}
