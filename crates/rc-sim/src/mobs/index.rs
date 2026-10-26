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
//! Switch: `RUSTCRAFT_OPT_MOBINDEX=0` returns the scan alone (old path), read once.

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

/// Hint id → slot and UUID → slot, rebuilt when it has been served wrong.
#[derive(Default)]
pub struct MobIndex {
    by_id: hashbrown::HashMap<i32, u32>,
    by_uuid: hashbrown::HashMap<u128, u32>,
    /// Species → slots in the list's order. Valid as long as the list has `kind_len` slots and
    /// no mutation outside `pushed` is signalled; assumption (checked at every query
    /// during tests): a slot only ever receives the mob that came out of it and
    /// the species of a mob does not change (conversions add a new mob).
    by_kind: hashbrown::HashMap<Kind, Vec<u32>>,
    kind_len: usize,
    kind_bad: bool,
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
        if kind_on() {
            // Direct addition went unnoticed (unexpected length): per-species index to redo.
            if !self.kind_bad && self.kind_len == list.len() {
                self.by_kind.entry(m.kind).or_default().push(at);
                self.kind_len += 1;
            } else {
                self.kind_bad = true;
            }
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
        let kinds = kind_on() && (shifted || self.kind_bad || self.kind_len != list.len() || *self.stale.get_mut());
        if ids || kinds {
            self.rebuild(list);
        }
    }

    pub(crate) fn rebuild(&mut self, list: &List) {
        self.by_id.clear();
        self.by_uuid.clear();
        self.by_kind.values_mut().for_each(Vec::clear);
        let (mut dup_id, mut dup_uuid) = (false, false);
        let (ids, kinds) = (on(), kind_on());
        for (i, m) in list.iter().enumerate() {
            if let Some(m) = m {
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
        }
        (self.dup_id, self.dup_uuid) = (dup_id, dup_uuid);
        (self.kind_len, self.kind_bad) = (list.len(), false);
        *self.stale.get_mut() = false;
    }

    /// Slots of species `k` in the list's order (empty slots possible for the duration of a
    /// tick: the caller skips them), or `None` if the index is not reliable (scan then).
    pub(crate) fn kind_slots(&self, list: &List, k: Kind) -> Option<&[u32]> {
        if !kind_on() || self.kind_bad || self.kind_len != list.len() || self.stale.load(Ordering::Relaxed) {
            return None;
        }
        Some(self.by_kind.get(&k).map_or(&[], Vec::as_slice))
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

/// Test counters: reads served by the hint, found by scan, absent.
#[cfg(test)]
pub(crate) mod stats {
    use std::sync::atomic::AtomicU64;
    pub(crate) static HIT: AtomicU64 = AtomicU64::new(0);
    pub(crate) static FOUND: AtomicU64 = AtomicU64::new(0);
    pub(crate) static ABSENT: AtomicU64 = AtomicU64::new(0);
}
