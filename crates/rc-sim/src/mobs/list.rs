//! The list of ticking mobs, in order of addition, behind a small API.
//!
//! The slots are private to this file: a mob is read with `get` / `iter`, changed in place with
//! `get_mut` / `iter_mut`, taken out of its slot with `take` and put back with `put`, added with
//! `Mobs::push`, and the list is compacted or re-sorted only by the `Mobs` methods written here.
//! Every change of a slot, or of a mob in place, therefore goes through the few functions of this
//! file: the compiler guarantees that no site is forgotten, which is what an exact spatial index of
//! the mobs needs (it follows these functions instead of guessing which mob moved). Reads never
//! change a slot, so `slots` hands the raw slots out read-only (walks by slot number, index
//! rebuilds).

use super::Mobs;
use super::index;
use super::mob::Mob;
use super::species::Kind;

/// Slots of the mobs in order of addition; a slot is empty while its mob is taken out.
#[derive(Default)]
pub struct MobList {
    slots: Vec<Option<Box<Mob>>>,
}

impl MobList {
    /// Number of slots, empty ones included.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Mob of slot `slot` (none if the slot is empty or past the end).
    #[inline]
    pub(crate) fn get(&self, slot: usize) -> Option<&Mob> {
        self.slots.get(slot)?.as_deref()
    }

    /// Mob of slot `slot`, to change in place.
    #[inline]
    pub(crate) fn get_mut(&mut self, slot: usize) -> Option<&mut Mob> {
        self.slots.get_mut(slot)?.as_deref_mut()
    }

    /// Mob of slot `slot` taken out (for its tick, or to hand it to the server with `&mut self`);
    /// the slot stays empty until `put`.
    #[inline]
    pub(crate) fn take(&mut self, slot: usize) -> Option<Box<Mob>> {
        self.slots.get_mut(slot)?.take()
    }

    /// `m` put (back) into slot `slot`, replacing what is there. Panics past the end, like an index.
    #[inline]
    pub(crate) fn put(&mut self, slot: usize, m: Box<Mob>) {
        self.slots[slot] = Some(m);
    }

    /// `put` if the slot still exists; otherwise the mob is handed back (the list got shorter
    /// while the mob was out).
    #[inline]
    pub(crate) fn try_put(&mut self, slot: usize, m: Box<Mob>) -> Result<(), Box<Mob>> {
        match self.slots.get_mut(slot) {
            Some(s) => {
                *s = Some(m);
                Ok(())
            }
            None => Err(m),
        }
    }

    /// Mobs in the list's order, empty slots skipped.
    #[inline]
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &Mob> + Clone {
        self.slots.iter().filter_map(|s| s.as_deref())
    }

    /// Mobs in the list's order, empty slots skipped, to change in place.
    #[inline]
    pub(crate) fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut Mob> {
        self.slots.iter_mut().filter_map(|s| s.as_deref_mut())
    }

    /// `update` applied to every mob in place, the list cut into `runs` contiguous runs shared
    /// between the calling thread and the helper threads. Only for an update that reads and
    /// writes its own mob and nothing else: the order between mobs is not defined.
    pub(crate) fn update_in_parallel(&mut self, runs: usize, update: impl Fn(&mut Mob) + Sync) {
        let run_len = self.slots.len().div_ceil(runs.max(1)).max(1);
        let update = &update;
        let jobs: Vec<_> = self
            .slots
            .chunks_mut(run_len)
            .map(|run| move || run.iter_mut().flatten().for_each(|m| update(m.as_mut())))
            .collect();
        crate::helper_pool::run_in_order(crate::helper_pool::logical_cores(), jobs);
    }

    /// The raw slots, read-only (walks that need the slot number or the empty slots).
    #[inline]
    pub(crate) fn slots(&self) -> &[Option<Box<Mob>>] {
        &self.slots
    }

    /// Tests of the index: a mob added at the end WITHOUT telling the index (`Mobs::push` does).
    #[cfg(test)]
    pub(crate) fn push_unindexed(&mut self, m: Option<Box<Mob>>) {
        self.slots.push(m);
    }
}

impl Mobs {
    /// Added at the end of the list (index kept up to date), with its rank in the entity walk.
    pub(crate) fn push(&mut self, mut m: Box<Mob>) {
        m.rank = crate::walk::next_rank();
        self.index.pushed(&m, &self.list.slots);
        self.list.slots.push(Some(m));
    }

    /// Slot `slot` removed, the later ones shifted down by one (index to redo); its content is
    /// returned.
    pub(crate) fn remove_slot(&mut self, slot: usize) -> Option<Box<Mob>> {
        let m = self.list.slots.remove(slot);
        self.index.mark_stale();
        m
    }

    /// Emptied slots dropped from the list (end of the mob tick), index brought up to date: the
    /// slots before the first emptied one do not move, only the rest is re-indexed.
    pub(crate) fn drop_empty_slots(&mut self) {
        let list = &mut self.list.slots;
        let Some(first_gap) = list.iter().position(Option::is_none) else {
            self.index.refresh(list, false);
            return;
        };
        let len_before = list.len();
        list.retain(Option::is_some);
        self.index.refresh_from(list, first_gap, len_before);
    }

    /// List put back in rank order (stable), index brought up to date from the first slot the sort
    /// moves.
    pub(crate) fn sort_by_rank(&mut self) {
        let list = &mut self.list.slots;
        let rank = |m: &Option<Box<Mob>>| m.as_ref().map_or(0, |m| m.rank);
        let first_moved = first_out_of_order(list, rank);
        crate::walk::sort_by_rank(list, rank);
        let len = list.len();
        self.index.refresh_from(list, first_moved.unwrap_or(len), len);
    }

    /// `of_kinds`, mutable: same mobs in the same order as
    /// `list.iter_mut().filter(|o| kinds.contains(&o.kind))`.
    pub(crate) fn of_kinds_mut<const N: usize>(&mut self, kinds: [Kind; N]) -> impl Iterator<Item = &mut Mob> {
        let Mobs { list, index, .. } = self;
        let list = &mut list.slots;
        let slots = index.kinds_slots(list, kinds);
        #[cfg(test)]
        if index::verify() {
            super::check_species_slots(list, slots.clone(), kinds);
        }
        let (indexed, all) = match slots {
            Some(s) => (SlotsMut { rest: &mut list[..], next_slot: 0, slots: s }, &mut [][..]),
            None => (SlotsMut { rest: &mut [][..], next_slot: 0, slots: index::MergedSlots::empty() }, &mut list[..]),
        };
        indexed
            .chain(all.iter_mut())
            .filter_map(|o| o.as_deref_mut())
            .filter(move |o| kinds.contains(&o.kind))
    }
}

/// First slot a stable sort by `key` moves: the first one whose key is above a later one. Every
/// slot before it is at most every later key, so a stable sort leaves it in place.
fn first_out_of_order<T>(list: &[T], key: impl Fn(&T) -> u64) -> Option<usize> {
    let mut first = None;
    let mut min_after = u64::MAX;
    for (i, x) in list.iter().enumerate().rev() {
        let k = key(x);
        if k > min_after {
            first = Some(i);
        }
        min_after = min_after.min(k);
    }
    first
}

/// Mutable walk over increasing slots of the list: each slot is split off the remaining tail, so
/// the borrows handed out never overlap.
struct SlotsMut<'a, I> {
    rest: &'a mut [Option<Box<Mob>>],
    /// Slot number of `rest[0]`.
    next_slot: usize,
    slots: I,
}

impl<'a, I: Iterator<Item = usize>> Iterator for SlotsMut<'a, I> {
    type Item = &'a mut Option<Box<Mob>>;

    fn next(&mut self) -> Option<Self::Item> {
        let s = self.slots.next()?;
        let rest = std::mem::take(&mut self.rest);
        let (slot, rest) = rest.get_mut(s.checked_sub(self.next_slot)?..)?.split_first_mut()?;
        self.rest = rest;
        self.next_slot = s + 1;
        Some(slot)
    }
}
