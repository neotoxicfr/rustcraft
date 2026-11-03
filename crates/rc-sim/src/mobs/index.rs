//! INDICATIVE index of mobs: identifier → slot of `Mobs::list`, UUID → slot.
//!
//! The list remains the only truth (public field, dozens of sites take it out slot by slot
//! and put it back). The index is only a hint: each read checks that the designated slot
//! does carry the requested identifier; otherwise (slot empty for the duration of a tick, slot replaced,
//! list shifted by a removal, direct addition without `Mobs::push`) the earlier linear scan is redone
//! and the index is marked stale, rebuilt at the end of the mob tick. Forgetting a
//! mutation therefore costs time, never correctness.
//!
//! Assumption (checked at every search during tests): an identifier appears only
//! once in the list; the index keeps its FIRST slot, like the scan.
//!
//! Switches, each read once: `RUSTCRAFT_OPT_MOBINDEX=0` returns the scan alone (old path);
//! `RUSTCRAFT_OPT_KINDINDEX=0` turns the per-species index off (`of_kind`, `of_kinds` and their
//! mutable forms scan the list); `RUSTCRAFT_OPT_INDEXTAIL=0` rebuilds the whole index after a
//! removal instead of only the slots from the first removed one on.

use std::sync::atomic::{AtomicBool, Ordering};

use super::mob::Mob;
use super::species::Kind;

/// Index in service (default); `RUSTCRAFT_OPT_MOBINDEX=0`: linear scan alone.
pub(crate) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_MOBINDEX") != Some("0"))
}

/// Per-species index in service (default); `RUSTCRAFT_OPT_KINDINDEX=0`: scan alone.
pub(crate) fn kind_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_KINDINDEX") != Some("0"))
}

/// Re-index only the moved tail after a removal (default); `RUSTCRAFT_OPT_INDEXTAIL=0`: whole rebuild.
pub(crate) fn tail_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_INDEXTAIL") != Some("0"))
}

/// Slots of several species in increasing order, that is in the list's order. Each species keeps its
/// own ascending slot list and a slot holds one species, so the lists are disjoint: taking the
/// smallest head each time merges them without sorting.
#[derive(Clone)]
pub(crate) struct MergedSlots<'a, const N: usize> {
    heads: [&'a [u32]; N],
}

impl<const N: usize> MergedSlots<'_, N> {
    /// No slot (the caller scans the list instead).
    pub(crate) fn empty() -> Self {
        MergedSlots { heads: [&[]; N] }
    }
}

impl<const N: usize> Iterator for MergedSlots<'_, N> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        // One species (`of_kind`): nothing to merge, a plain walk of its list.
        if N == 1 {
            let (&first, rest) = self.heads[0].split_first()?;
            self.heads[0] = rest;
            return Some(first as usize);
        }
        let mut best: Option<usize> = None;
        for (j, h) in self.heads.iter().enumerate() {
            if let Some(&s) = h.first()
                && best.is_none_or(|b| s < self.heads[b][0])
            {
                best = Some(j);
            }
        }
        let j = best?;
        let (&first, rest) = self.heads[j].split_first()?;
        self.heads[j] = rest;
        Some(first as usize)
    }
}

/// Hint id → slot and UUID → slot, rebuilt when it has been served wrong.
#[derive(Default)]
pub struct MobIndex {
    by_id: hashbrown::HashMap<i32, u32>,
    by_uuid: hashbrown::HashMap<u128, u32>,
    /// Species → slots in the list's order. Valid as long as the list has `indexed_len` slots and
    /// no mutation outside `pushed` is signalled; assumption (checked at every query
    /// during tests): a slot only ever receives the mob that came out of it and
    /// the species of a mob does not change (conversions add a new mob).
    by_kind: hashbrown::HashMap<Kind, Vec<u32>>,
    /// Length of the list as the index saw it (last rebuild, then each `pushed`), kept whatever the
    /// switches: a list longer than this got a mob the index never saw.
    indexed_len: usize,
    /// A mob was added without `Mobs::push`: entries are missing until the next whole rebuild.
    missed_addition: bool,
    /// A search found by scanning what the hint did not give.
    stale: AtomicBool,
    /// Value seen twice (two living slots): searches of this kind by scan
    /// until the next rebuild without a duplicate (first slot guaranteed).
    dup_id: bool,
    dup_uuid: bool,
}

type List = Vec<Option<Box<Mob>>>;

impl MobIndex {
    /// Mob added at the end of the list at slot `at`.
    pub(crate) fn pushed(&mut self, m: &Mob, list: &List) {
        let at = list.len() as u32;
        // Direct addition went unnoticed (unexpected length): index to redo in full.
        if !self.missed_addition && self.indexed_len == list.len() {
            self.indexed_len += 1;
            if kind_on() {
                self.by_kind.entry(m.kind).or_default().push(at);
            }
        } else {
            self.missed_addition = true;
        }
        if !on() {
            return;
        }
        // An already indexed value keeps its slot (the first, like the scan) and signals a
        // duplicate; a stale entry will be seen as wrong on reading then rebuilt.
        if Self::insert(&mut self.by_id, m.id, at) {
            self.dup_id = true;
        }
        if m.uuid != 0 && Self::insert(&mut self.by_uuid, m.uuid, at) {
            self.dup_uuid = true;
        }
    }

    /// True if the key was already there (possible duplicate).
    fn insert<K: std::hash::Hash + Eq>(map: &mut hashbrown::HashMap<K, u32>, k: K, at: u32) -> bool {
        match map.entry(k) {
            hashbrown::hash_map::Entry::Occupied(_) => true,
            hashbrown::hash_map::Entry::Vacant(v) => {
                v.insert(at);
                false
            }
        }
    }

    /// No identifier seen twice (index in service): a slot that carries the identifier
    /// is then the first.
    pub(crate) fn unique_ids(&self) -> bool {
        on() && !self.dup_id
    }

    /// Hint to redo (list shifted, mutation outside `Mobs::push`).
    pub(crate) fn mark_stale(&self) {
        self.stale.store(true, Ordering::Relaxed);
    }

    /// Rebuild if the hint has been served wrong or if `shifted` (slots removed).
    pub(crate) fn refresh(&mut self, list: &List, shifted: bool) {
        let ids = on() && (shifted || self.dup_id || self.dup_uuid || *self.stale.get_mut());
        let kinds =
            kind_on() && (shifted || self.missed_addition || self.indexed_len != list.len() || *self.stale.get_mut());
        if ids || kinds {
            self.rebuild(list);
        }
    }

    /// Slots from `first_moved` on changed (removed slots dropped, or a sort), the slots before it
    /// kept their mob; `len_before`: length of the list before the change. Only the moved tail is
    /// re-indexed when the rest of the index is in a state a whole rebuild would not improve
    /// (no duplicate, nothing served wrong, per-species index matching the old length).
    pub(crate) fn refresh_from(&mut self, list: &List, first_moved: usize, len_before: usize) {
        // Dropping the tail's entries one by one costs more than clearing the maps once the tail is
        // the larger part (bench: half the crowd removed from slot 1 on, 19 µs against 7).
        let tail_is_short = first_moved >= list.len() / 2;
        if !tail_on() || first_moved == 0 || !tail_is_short || !self.tail_reusable(len_before) {
            #[cfg(test)]
            stats::count_refresh(false);
            self.refresh(list, true);
            return;
        }
        #[cfg(test)]
        stats::count_refresh(true);
        self.reindex_tail(list, first_moved);
        #[cfg(test)]
        if verify() {
            self.check_tail(list, first_moved);
        }
    }

    /// The entries before the tail are those a whole rebuild would give: no duplicate, nothing
    /// served wrong, and every mob of the old list seen by the index (checked whatever the
    /// switches, since an unseen mob before the tail would stay unindexed).
    fn tail_reusable(&mut self, len_before: usize) -> bool {
        let ids_ok = !on() || !(self.dup_id || self.dup_uuid);
        let all_seen = !self.missed_addition && self.indexed_len == len_before;
        ids_ok && all_seen && !*self.stale.get_mut()
    }

    /// Entries of the slots from `first` on dropped, then those slots indexed again.
    fn reindex_tail(&mut self, list: &List, first: usize) {
        let first32 = first as u32;
        if on() {
            // A removed mob leaves no entry: every entry at `first` or after goes, then the
            // mobs still there come back at their new slot.
            self.by_id.retain(|_, s| *s < first32);
            self.by_uuid.retain(|_, s| *s < first32);
        }
        if kind_on() {
            // Each list is ascending: the slots kept are a prefix.
            for v in self.by_kind.values_mut() {
                v.truncate(v.partition_point(|&s| s < first32));
            }
        }
        self.index_slots_from(list, first);
    }

    pub(crate) fn rebuild(&mut self, list: &List) {
        self.by_id.clear();
        self.by_uuid.clear();
        self.by_kind.values_mut().for_each(Vec::clear);
        self.index_slots_from(list, 0);
        self.missed_addition = false;
        *self.stale.get_mut() = false;
    }

    /// Adds the mobs of the slots from `first` on (their entries are absent) and records the
    /// length seen; duplicates are judged on these slots, the ones before carrying none.
    fn index_slots_from(&mut self, list: &List, first: usize) {
        let (ids, kinds) = (on(), kind_on());
        let (mut dup_id, mut dup_uuid) = (false, false);
        for (i, m) in list.iter().enumerate().skip(first) {
            let Some(m) = m else { continue };
            if ids {
                dup_id |= Self::insert(&mut self.by_id, m.id, i as u32);
                if m.uuid != 0 {
                    dup_uuid |= Self::insert(&mut self.by_uuid, m.uuid, i as u32);
                }
            }
            if kinds {
                self.by_kind.entry(m.kind).or_default().push(i as u32);
            }
        }
        (self.dup_id, self.dup_uuid) = (dup_id, dup_uuid);
        self.indexed_len = list.len();
    }

    /// Slots of species `k` in the list's order (empty slots possible for the duration of a
    /// tick: the caller skips them), or `None` if the index is not reliable (scan then).
    /// (Tests and benches; the server goes through `kinds_slots`.)
    #[cfg(test)]
    pub(crate) fn kind_slots(&self, list: &List, k: Kind) -> Option<&[u32]> {
        self.kind_index_reliable(list).then(|| self.slots_of(k))
    }

    /// Slots of the species `kinds` merged in the list's order, or `None` if the index is not
    /// reliable. A species named twice is walked once.
    pub(crate) fn kinds_slots<const N: usize>(&self, list: &List, kinds: [Kind; N]) -> Option<MergedSlots<'_, N>> {
        if !self.kind_index_reliable(list) {
            return None;
        }
        let mut heads: [&[u32]; N] = [&[]; N];
        for (j, &k) in kinds.iter().enumerate() {
            if !kinds[..j].contains(&k) {
                heads[j] = self.slots_of(k);
            }
        }
        Some(MergedSlots { heads })
    }

    fn slots_of(&self, k: Kind) -> &[u32] {
        self.by_kind.get(&k).map_or(&[], Vec::as_slice)
    }

    /// Per-species index in step with `list` (counted under test: served or fallback scan).
    fn kind_index_reliable(&self, list: &List) -> bool {
        let reliable =
            kind_on() && !self.missed_addition && self.indexed_len == list.len() && !self.stale.load(Ordering::Relaxed);
        #[cfg(test)]
        stats::count_kind_query(reliable);
        reliable
    }

    /// First slot of `list` carrying the identifier `id`.
    #[inline]
    pub(crate) fn find(&self, list: &List, id: i32) -> Option<usize> {
        let r = self.find_fast(list, id);
        #[cfg(test)]
        if verify() {
            assert_eq!(r, scan_id(list, id), "mob index: id {id} wrong");
        }
        r
    }

    fn find_fast(&self, list: &List, id: i32) -> Option<usize> {
        if !on() || self.dup_id {
            return scan_id(list, id);
        }
        if let Some(&i) = self.by_id.get(&id)
            && list.get(i as usize).and_then(Option::as_ref).is_some_and(|m| m.id == id)
        {
            #[cfg(test)]
            stats::HIT.fetch_add(1, Ordering::Relaxed);
            return Some(i as usize);
        }
        let r = scan_id(list, id);
        #[cfg(test)]
        if r.is_some() { &stats::FOUND } else { &stats::ABSENT }.fetch_add(1, Ordering::Relaxed);
        if r.is_some() {
            self.mark_stale();
        }
        r
    }

    /// First slot of `list` carrying the UUID `u` (never 0).
    #[inline]
    pub(crate) fn find_uuid(&self, list: &List, u: u128) -> Option<usize> {
        let r = self.find_uuid_fast(list, u);
        #[cfg(test)]
        if verify() {
            assert_eq!(r, scan_uuid(list, u), "mob index: UUID {u:x} wrong");
        }
        r
    }

    fn find_uuid_fast(&self, list: &List, u: u128) -> Option<usize> {
        if !on() || self.dup_uuid {
            return scan_uuid(list, u);
        }
        if let Some(&i) = self.by_uuid.get(&u)
            && list.get(i as usize).and_then(Option::as_ref).is_some_and(|m| m.uuid == u)
        {
            return Some(i as usize);
        }
        let r = scan_uuid(list, u);
        if r.is_some() {
            self.mark_stale();
        }
        r
    }
}

#[cfg(test)]
impl MobIndex {
    /// Raw (unchecked) hint of an identifier.
    pub(crate) fn raw_hint(&self, id: i32) -> Option<usize> {
        self.by_id.get(&id).map(|&i| i as usize)
    }

    /// Same entries as an index rebuilt from scratch on `list` (empty per-species lists ignored).
    pub(crate) fn assert_same_as_rebuilt(&self, list: &List, what: &str) {
        let mut fresh = MobIndex::default();
        fresh.rebuild(list);
        let kinds = |ix: &MobIndex| {
            let mut v: Vec<(Kind, Vec<u32>)> =
                ix.by_kind.iter().filter(|(_, s)| !s.is_empty()).map(|(k, s)| (*k, s.clone())).collect();
            v.sort_by_key(|(k, _)| *k as u32);
            v
        };
        assert_eq!(self.by_id, fresh.by_id, "{what}: id entries");
        assert_eq!(self.by_uuid, fresh.by_uuid, "{what}: UUID entries");
        assert_eq!(kinds(self), kinds(&fresh), "{what}: per-species entries");
        let flags = |ix: &MobIndex| (ix.dup_id, ix.dup_uuid, ix.indexed_len, ix.missed_addition);
        assert_eq!(flags(self), flags(&fresh), "{what}");
    }

    /// After a tail re-index: every mob from `first` on is found at its slot by the hint (unless a
    /// duplicate turned the hint off), every per-species list is ascending, within the list and
    /// holds every tail mob of its species, and nothing points past the end.
    fn check_tail(&self, list: &List, first: usize) {
        let len = list.len() as u32;
        if on() {
            assert!(self.by_id.values().chain(self.by_uuid.values()).all(|&s| s < len), "mob index: entry past the end");
        }
        for (i, m) in list.iter().enumerate().skip(first) {
            let Some(m) = m else { continue };
            if on() && !self.dup_id {
                assert_eq!(self.by_id.get(&m.id), Some(&(i as u32)), "mob index: tail id {} not re-indexed", m.id);
            }
            if on() && !self.dup_uuid && m.uuid != 0 {
                assert_eq!(self.by_uuid.get(&m.uuid), Some(&(i as u32)), "mob index: tail UUID not re-indexed");
            }
            if kind_on() {
                assert!(self.slots_of(m.kind).binary_search(&(i as u32)).is_ok(), "per-species index: tail slot {i} missing");
            }
        }
        if kind_on() {
            for v in self.by_kind.values() {
                assert!(v.windows(2).all(|w| w[0] < w[1]) && v.last().is_none_or(|&s| s < len), "per-species index: bad list");
            }
        }
    }
}

/// Old path (and test reference): scan in the list's order.
pub(crate) fn scan_id(list: &List, id: i32) -> Option<usize> {
    list.iter().position(|o| o.as_ref().is_some_and(|o| o.id == id))
}

pub(crate) fn scan_uuid(list: &List, u: u128) -> Option<usize> {
    list.iter().position(|o| o.as_ref().is_some_and(|o| o.uuid == u))
}

// Tests: each indexed search compared with the scan. Specific to the thread: a measurement bench that
// turns it off does not turn it off for the tests running in parallel on other threads.
#[cfg(test)]
thread_local! {
    static VERIFY: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

#[cfg(test)]
pub(crate) fn verify() -> bool {
    VERIFY.with(std::cell::Cell::get)
}

/// Measurement benches: turns the check off (or back on) for the current thread only.
#[cfg(test)]
pub(crate) fn set_verify(on: bool) {
    VERIFY.with(|v| v.set(on));
}

/// Test counters: reads served by the hint, found by scan, absent; per-species queries served by
/// the index or sent back to the scan (index out of step, e.g. after a direct addition); refreshes
/// after a removal or a sort, by tail re-index or by whole rebuild.
#[cfg(test)]
pub(crate) mod stats {
    use std::cell::Cell;
    use std::sync::atomic::AtomicU64;
    pub(crate) static HIT: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FOUND: AtomicU64 = AtomicU64::new(0);
    pub(crate) static ABSENT: AtomicU64 = AtomicU64::new(0);

    // Specific to the thread, so that a test counts only its own server's work.
    thread_local! {
        static KIND_QUERIES: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
        static REFRESHES: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }

    /// Adds one to the first (`yes`) or the second count of the pair.
    fn bump(pair: &'static std::thread::LocalKey<Cell<(u64, u64)>>, yes: bool) {
        pair.with(|c| {
            let (a, b) = c.get();
            c.set(if yes { (a + 1, b) } else { (a, b + 1) });
        });
    }

    pub(super) fn count_kind_query(served: bool) {
        bump(&KIND_QUERIES, served);
    }

    pub(super) fn count_refresh(tail: bool) {
        bump(&REFRESHES, tail);
    }

    /// Per-species queries of this thread so far: (served by the index, sent to the scan).
    pub(crate) fn kind_queries() -> (u64, u64) {
        KIND_QUERIES.with(Cell::get)
    }

    /// Refreshes after a removal or a sort on this thread so far: (tail re-index, whole rebuild).
    pub(crate) fn refreshes() -> (u64, u64) {
        REFRESHES.with(Cell::get)
    }
}
