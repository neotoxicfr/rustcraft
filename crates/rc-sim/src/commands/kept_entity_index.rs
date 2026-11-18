//! Hint identifier → (chunk, list index) for the kept entity compounds (`Server::entities`:
//! markers, armor stands, frames, every entity the simulation keeps as NBT).
//!
//! The lists stay the only truth and change in many places (chunk loads, adoptions, summons,
//! kills, refiles), so the index is only a hint: a read checks that the hinted slot still holds
//! the identifier, otherwise it searches the hinted chunk's list (a removal earlier in it only
//! shifts the entry towards the front), then every list as before, and repairs that one entry.
//! Kills and moves to another chunk tell the index directly; each dimension keeps its own index
//! (swapped with its lists). A change the index never heard of costs time, never a wrong answer:
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

/// Hint of where each kept compound sits, each wrong entry repaired by the read that meets it.
#[derive(Default)]
pub(crate) struct KeptEntityIndex {
    slots: RefCell<hashbrown::HashMap<i32, (Chunk, u32)>>,
    /// Made from every list once, at the first read; afterwards only single entries change.
    built: Cell<bool>,
    /// UUID → smallest identifier carrying it, made at the first lookup by UUID (most worlds
    /// never ask, and reading every compound's UUID costs more than its slot); `None`: to make.
    by_uuid: RefCell<Option<hashbrown::HashMap<u128, i32>>>,
    /// Tests: walks over every list (the first build and the scans), what a hint should save.
    #[cfg(test)]
    full_walks: Cell<u32>,
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
        if !self.built.get() {
            self.build(lists);
        }
        let hint = self.slots.borrow().get(&id).copied();
        if let Some((chunk, k)) = hint {
            let list = lists.get(&chunk).map_or(&[][..], Vec::as_slice);
            if list.get(k as usize).is_some_and(|(x, _)| *x == id) {
                return Some((chunk, k as usize));
            }
            // Shifted towards the front by a removal earlier in its own list: that list alone
            // is searched (never more than the old scan) and only this entry is repaired. A
            // rebuild here cost one per marker of `kill @e[type=marker]` on a shared chunk.
            if let Some(k) = list.iter().position(|(x, _)| *x == id) {
                self.slots.borrow_mut().insert(id, (chunk, k as u32));
                return Some((chunk, k));
            }
        }
        let found = self.scan_counted(lists, id);
        let mut slots = self.slots.borrow_mut();
        match found {
            // Added or moved behind the index's back: only this entry was missing or wrong.
            Some((chunk, k)) => slots.insert(id, (chunk, k as u32)),
            None => slots.remove(&id),
        };
        found
    }

    fn build(&self, lists: &Lists) {
        #[cfg(test)]
        self.full_walks.set(self.full_walks.get() + 1);
        let mut slots = self.slots.borrow_mut();
        slots.clear();
        for (chunk, list) in lists {
            for (k, (id, _)) in list.iter().enumerate() {
                // First one in the scan's order, should an identifier ever repeat.
                slots.entry(*id).or_insert((*chunk, k as u32));
            }
        }
        self.built.set(true);
    }

    fn scan_counted(&self, lists: &Lists, id: i32) -> Option<(Chunk, usize)> {
        #[cfg(test)]
        self.full_walks.set(self.full_walks.get() + 1);
        scan(lists, id)
    }

    /// Kept compound `id` now sits at slot `k` of `chunk` (pushed there by a move).
    pub(crate) fn placed(&self, id: i32, chunk: Chunk, k: usize) {
        self.slots.borrow_mut().insert(id, (chunk, k as u32));
    }

    /// Kept compound `id`, carrying `uuid`, left the lists (killed): its entries go too, so the
    /// index holds only live compounds.
    pub(crate) fn forget(&self, id: i32, uuid: u128) {
        self.slots.borrow_mut().remove(&id);
        if let Some(map) = self.by_uuid.borrow_mut().as_mut()
            && map.get(&uuid) == Some(&id)
        {
            map.remove(&uuid);
        }
    }

    /// A kept compound's UUID changed (one without any was given one): the UUID map is made
    /// again, since a smaller identifier may now carry a UUID the map gives to a larger one.
    pub(crate) fn uuid_changed(&self) {
        *self.by_uuid.borrow_mut() = None;
    }

    /// Tests: how many times every list was walked so far.
    #[cfg(test)]
    pub(crate) fn full_walks(&self) -> u32 {
        self.full_walks.get()
    }

    /// Tests: the stored hint of `id`, unchecked (what a read must never trust blindly).
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
