//! Hint identifier → (chunk, list index) for the kept entity compounds (`Server::entities`:
//! markers, armor stands, frames, every entity the simulation keeps as NBT).
//!
//! The lists stay the only truth and change in many places (chunk loads and unloads, summons,
//! kills, refiles, dimension swaps), so the index is only a hint: a read checks that the hinted
//! slot still holds the identifier, otherwise it scans the lists as before and repairs the hint
//! (the one entry when it was missing, a whole rebuild at the next read when a list shifted under
//! it). A change the index never heard of costs time, never a wrong answer:
//! one counter numbers every entity (`Server::next_entity`), so a slot holding the identifier is
//! the one the scan finds (checked against the scan at every read during tests).
//!
//! What it saves: `@s`, `scoreboard players … @s` and every other lookup of a marker by its
//! identifier walked every kept compound of the dimension (1,000 markers running a function:
//! quadratic per tick). An identifier that is not a kept compound (a minecart, an End crystal)
//! still costs the scan, as before.
//!
//! `RUSTCRAFT_OPT_KEPT_ENTITY_INDEX=0`: the scan alone (old path).

use std::cell::{Cell, RefCell};

use rc_proto::nbt::Nbt;

type Chunk = (i32, i32);
type Lists = hashbrown::HashMap<Chunk, Vec<(i32, Nbt)>>;

/// Index in service (default); `RUSTCRAFT_OPT_KEPT_ENTITY_INDEX=0`: linear scan alone.
fn on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_KEPT_ENTITY_INDEX")
}

/// Hint of where each kept compound sits, rebuilt from the lists when it served a wrong slot.
#[derive(Default)]
pub(crate) struct KeptEntityIndex {
    slots: RefCell<hashbrown::HashMap<i32, (Chunk, u32)>>,
    /// Built from the lists and not caught out since (false: rebuild at the next read).
    fresh: Cell<bool>,
    /// UUID → smallest identifier carrying it, made at the first lookup by UUID (most worlds
    /// never ask, and reading every compound's UUID costs more than its slot); `None`: to make.
    by_uuid: RefCell<Option<hashbrown::HashMap<u128, i32>>>,
}

impl KeptEntityIndex {
    /// Smallest identifier of the kept compounds carrying UUID `u` (never 0): the score holder
    /// named in every `scoreboard players … @s` message of a marker read every compound's UUID.
    pub(crate) fn find_uuid(&self, lists: &Lists, u: u128) -> Option<i32> {
        let found = if on() { self.find_uuid_hinted(lists, u) } else { scan_uuid(lists, u) };
        #[cfg(test)]
        if crate::mobs::index::verify() {
            assert_eq!(found, scan_uuid(lists, u), "kept entity index: UUID {u:x} wrong");
        }
        found
    }

    fn find_uuid_hinted(&self, lists: &Lists, u: u128) -> Option<i32> {
        let hint = self.by_uuid.borrow_mut().get_or_insert_with(|| uuid_map(lists)).get(&u).copied();
        // Checked: the hinted compound is still there and still carries `u`. Compounds added
        // since the map was made have larger identifiers (one counter), so it stays the smallest.
        if let Some(id) = hint
            && self.find(lists, id).is_some_and(|(c, k)| compound_uuid(&lists[&c][k].1) == Some(u))
        {
            return Some(id);
        }
        let found = scan_uuid(lists, u);
        if found.is_some() {
            // Added or changed behind the map's back: made again at the next lookup by UUID.
            *self.by_uuid.borrow_mut() = None;
        }
        found
    }

    /// Chunk and list index of kept compound `id`: the first one the scan of `lists` finds.
    pub(crate) fn find(&self, lists: &Lists, id: i32) -> Option<(Chunk, usize)> {
        let found = if on() { self.find_hinted(lists, id) } else { scan(lists, id) };
        #[cfg(test)]
        if crate::mobs::index::verify() {
            assert_eq!(found, scan(lists, id), "kept entity index: id {id} wrong");
        }
        found
    }

    fn find_hinted(&self, lists: &Lists, id: i32) -> Option<(Chunk, usize)> {
        if !self.fresh.get() {
            self.rebuild(lists);
        }
        let hint = self.slots.borrow().get(&id).copied();
        if let Some((chunk, k)) = hint
            && lists.get(&chunk).and_then(|l| l.get(k as usize)).is_some_and(|(x, _)| *x == id)
        {
            return Some((chunk, k as usize));
        }
        let found = scan(lists, id);
        match (hint, found) {
            // Shifted by a removal or moved: the entries after it in its list are likely wrong
            // too, one rebuild at the next read fixes them all.
            (Some(_), Some(_)) => self.fresh.set(false),
            // Added behind the index's back: only this entry is missing.
            (None, Some((chunk, k))) => {
                self.slots.borrow_mut().insert(id, (chunk, k as u32));
            }
            (Some(_), None) => {
                self.slots.borrow_mut().remove(&id);
            }
            (None, None) => {}
        }
        found
    }

    fn rebuild(&self, lists: &Lists) {
        let mut slots = self.slots.borrow_mut();
        slots.clear();
        for (chunk, list) in lists {
            for (k, (id, _)) in list.iter().enumerate() {
                // First one in the scan's order, should an identifier ever repeat.
                slots.entry(*id).or_insert((*chunk, k as u32));
            }
        }
        self.fresh.set(true);
    }

    /// Tests: the rebuilt hint of `id`, unchecked (what a read must never trust blindly).
    #[cfg(test)]
    pub(crate) fn raw_hint(&self, id: i32) -> Option<(Chunk, usize)> {
        self.slots.borrow().get(&id).map(|&(c, k)| (c, k as usize))
    }
}

/// Old path (and test reference): the lists walked in the map's order.
pub(crate) fn scan(lists: &Lists, id: i32) -> Option<(Chunk, usize)> {
    lists.iter().find_map(|(c, l)| l.iter().position(|(i, _)| *i == id).map(|k| (*c, k)))
}

/// Old path (and test reference): smallest identifier of the compounds carrying `u`.
pub(crate) fn scan_uuid(lists: &Lists, u: u128) -> Option<i32> {
    lists.values().flatten().filter(|(_, n)| compound_uuid(n) == Some(u)).map(|(id, _)| *id).min()
}

fn uuid_map(lists: &Lists) -> hashbrown::HashMap<u128, i32> {
    let mut map = hashbrown::HashMap::new();
    for (id, n) in lists.values().flatten() {
        if let Some(u) = compound_uuid(n) {
            map.entry(u).and_modify(|best: &mut i32| *best = (*best).min(*id)).or_insert(*id);
        }
    }
    map
}

fn compound_uuid(n: &Nbt) -> Option<u128> {
    crate::nbtio::uuid(n.get("UUID"))
}
