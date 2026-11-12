//! Functions waiting for their tick (`/schedule`), kept with the world in
//! `data/minecraft/scheduled_events.dat` like the game's server data. An entry has an id (what
//! `clear` and `append` compare: `ns:path`, or `#ns:path` for a tag) and what it runs (the same
//! text, except in a file written by another tool). Like the game:
//! - an id already waiting for the same tick is not added twice (`append` included);
//! - the due entries are taken one at a time, so a due function that clears or replaces another
//!   entry due at the same tick stops it;
//! - an entry taken after its due tick (read back overdue) leaves its id behind: `clear` still
//!   counts it and the suggestions still show it, until it is cleared or the world is reopened.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use hashbrown::HashMap;
use rc_proto::nbt::Nbt;

/// One pending entry.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    id: String,
    /// What it runs when that is not `id` itself (only a file written by another tool).
    other_target: Option<Box<str>>,
}

impl Entry {
    fn target(&self) -> &str {
        self.other_target.as_deref().unwrap_or(&self.id)
    }

    fn into_target(self) -> String {
        self.other_target.map_or(self.id, String::from)
    }
}

/// Pending `/schedule` entries.
pub struct ScheduledFunctions {
    /// `RUSTCRAFT_OPT_SCHEDULEQUEUE`: entries ordered by (due tick, scheduling order) with an
    /// index of each id's keys, so a tick with nothing due reads one key and `clear` visits only
    /// its id; otherwise the former list, walked whole.
    ordered: bool,
    list: Vec<(i64, Entry)>,
    /// Former list: (id, due tick) of the entries taken overdue (see the module notes).
    overdue_ids: Vec<(String, i64)>,
    by_due: BTreeMap<(i64, u64), Entry>,
    /// Keys of each id: those in `by_due`, and those of entries taken overdue.
    keys_of: HashMap<String, Vec<(i64, u64)>>,
    next_order: u64,
    /// Changed since the file was read or last written: written at the next save. An absent or
    /// unreadable file counts as changed, so the first save writes it, like the game.
    dirty: bool,
}

impl ScheduledFunctions {
    pub fn new(ordered: bool) -> ScheduledFunctions {
        ScheduledFunctions {
            ordered,
            list: Vec::new(),
            overdue_ids: Vec::new(),
            by_due: BTreeMap::new(),
            keys_of: HashMap::new(),
            next_order: 0,
            dirty: true,
        }
    }

    /// Entries waiting to run.
    pub fn len(&self) -> usize {
        if self.ordered { self.by_due.len() } else { self.list.len() }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Schedules `id` (a function, or `#tag`) at `due`, after the entries already due then;
    /// nothing if `id` already waits for that tick.
    pub fn add(&mut self, due: i64, id: String) {
        self.add_entry(due, Entry { id, other_target: None });
    }

    fn add_entry(&mut self, due: i64, entry: Entry) {
        if self.ordered {
            let keys = self.keys_of.entry_ref(entry.id.as_str()).or_default();
            if keys.iter().any(|k| k.0 == due) {
                return;
            }
            let key = (due, self.next_order);
            self.next_order += 1;
            keys.push(key);
            self.by_due.insert(key, entry);
        } else {
            let waiting = self.list.iter().any(|(t, e)| *t == due && e.id == entry.id);
            if waiting || self.overdue_ids.iter().any(|(i, t)| *t == due && *i == entry.id) {
                return;
            }
            self.list.push((due, entry));
        }
        self.dirty = true;
    }

    /// Removes every entry of `id` (the ids left by overdue entries included); how many there were.
    pub fn remove_named(&mut self, id: &str) -> usize {
        let removed = if self.ordered {
            let keys = self.keys_of.remove(id).unwrap_or_default();
            for k in &keys {
                self.by_due.remove(k);
            }
            keys.len()
        } else {
            let before = self.list.len() + self.overdue_ids.len();
            self.list.retain(|(_, e)| e.id != id);
            self.overdue_ids.retain(|(i, _)| i != id);
            before - self.list.len() - self.overdue_ids.len()
        };
        self.dirty |= removed > 0;
        removed
    }

    /// Takes the next entry due at `now` or before (by due tick, then in scheduling order) and
    /// returns what it runs. One at a time: the caller runs it before taking the next one.
    pub fn pop_due(&mut self, now: i64) -> Option<String> {
        let entry = if self.ordered { self.pop_ordered(now)? } else { self.pop_listed(now)? };
        self.dirty = true;
        Some(entry.into_target())
    }

    fn pop_ordered(&mut self, now: i64) -> Option<Entry> {
        let first = self.by_due.first_entry().filter(|e| e.key().0 <= now)?;
        let (key, entry) = first.remove_entry();
        // The game forgets the id at the current tick only: an overdue entry keeps it.
        if key.0 == now {
            self.forget_key(&entry.id, key);
        }
        Some(entry)
    }

    fn pop_listed(&mut self, now: i64) -> Option<Entry> {
        // The first of the earliest due entries (the list is in scheduling order).
        let (i, _) = self.list.iter().enumerate().filter(|(_, (t, _))| *t <= now).min_by_key(|(_, (t, _))| *t)?;
        let (due, entry) = self.list.remove(i);
        if due < now {
            self.overdue_ids.push((entry.id.clone(), due));
        }
        Some(entry)
    }

    fn forget_key(&mut self, id: &str, key: (i64, u64)) {
        if let Some(keys) = self.keys_of.get_mut(id) {
            keys.retain(|k| *k != key);
            if keys.is_empty() {
                self.keys_of.remove(id);
            }
        }
    }

    /// The ids known to the queue, sorted, once each (suggestions of `schedule clear`).
    pub fn names(&self) -> Vec<String> {
        let ids: BTreeSet<&str> = if self.ordered {
            self.keys_of.keys().map(String::as_str).collect()
        } else {
            self.list.iter().map(|(_, e)| e.id.as_str()).chain(self.overdue_ids.iter().map(|(i, _)| i.as_str())).collect()
        };
        ids.into_iter().map(str::to_owned).collect()
    }

    /// The waiting entries in the order they run: (due tick, entry).
    fn in_run_order(&self) -> Vec<(i64, &Entry)> {
        if self.ordered {
            return self.by_due.iter().map(|((t, _), e)| (*t, e)).collect();
        }
        let mut v: Vec<(i64, &Entry)> = self.list.iter().map(|(t, e)| (*t, e)).collect();
        // Stable: entries due at the same tick keep their scheduling order.
        v.sort_by_key(|(t, _)| *t);
        v
    }

    /// The entries saved with the world, read back when it opens: added in the file's order (an
    /// id already waiting for the same tick skipped), an event the game would refuse skipped.
    /// Absent, unreadable or without an event list: empty queue, written at the next save.
    pub(crate) fn read_saved(&mut self, world: &Path) {
        let root = crate::nbtio::read_dat(&file_path(world)).ok();
        let Some(Nbt::List(events)) = root.as_ref().and_then(|r| r.get("data")?.get("events")) else {
            self.dirty = true;
            return;
        };
        for event in events {
            if let Some((due, entry)) = read_event(event) {
                self.add_entry(due, entry);
            }
        }
        self.dirty = false;
    }

    /// The file's write job when the queue changed since it was read or written.
    pub(crate) fn save_job(&mut self, world: &Path) -> Option<crate::save::Job> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let nbt = crate::nbtio::encode_root(&self.file_nbt());
        Some(crate::save::Job::Dat { path: file_path(world), nbt, backup: false })
    }

    /// The file's compound: the events in run order, like the game's.
    fn file_nbt(&self) -> Nbt {
        let s = |t: &str| Nbt::String(t.to_owned());
        let events = self
            .in_run_order()
            .into_iter()
            .map(|(due, e)| {
                let (kind, id) = match e.target().strip_prefix('#') {
                    Some(tag) => ("minecraft:function_tag", tag),
                    None => ("minecraft:function", e.target()),
                };
                Nbt::Compound(vec![
                    ("trigger_time".into(), Nbt::Long(due)),
                    ("id".into(), s(&e.id)),
                    ("callback".into(), Nbt::Compound(vec![("type".into(), s(kind)), ("id".into(), s(id))])),
                ])
            })
            .collect();
        Nbt::Compound(vec![
            ("data".into(), Nbt::Compound(vec![("events".into(), Nbt::List(events))])),
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
        ])
    }

    /// Every waiting entry: (due tick, id, what it runs), in run order.
    #[cfg(test)]
    pub(crate) fn entries(&self) -> Vec<(i64, String, String)> {
        self.in_run_order().into_iter().map(|(t, e)| (t, e.id.clone(), e.target().to_owned())).collect()
    }
}

/// `data/minecraft/scheduled_events.dat` of a world.
pub(crate) fn file_path(world: &Path) -> PathBuf {
    world.join("data").join("minecraft").join("scheduled_events.dat")
}

/// One saved event: due tick (integer), id (string), callback (function or function tag with a
/// valid identifier, written with or without its `minecraft:` namespace).
fn read_event(event: &Nbt) -> Option<(i64, Entry)> {
    let due = event.get("trigger_time")?.as_i64()?;
    let id = event.get("id")?.as_str()?.to_owned();
    let callback = event.get("callback")?;
    let target_id = identifier(callback.get("id")?.as_str()?)?;
    let target = match identifier(callback.get("type")?.as_str()?)?.as_str() {
        "minecraft:function" => target_id,
        "minecraft:function_tag" => format!("#{target_id}"),
        _ => return None,
    };
    let other_target = (target != id).then(|| target.into_boxed_str());
    Some((due, Entry { id, other_target }))
}

/// `ns:path` (`minecraft:` when the namespace is missing or empty); `None` if a character is not
/// allowed (namespace `a-z 0-9 _ . -`, path the same and `/`).
fn identifier(text: &str) -> Option<String> {
    let (namespace, path) = match text.split_once(':') {
        Some(("", path)) => ("minecraft", path),
        Some((namespace, path)) => (namespace, path),
        None => ("minecraft", text),
    };
    let allowed = |s: &str, slash: bool| {
        s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-') || (slash && b == b'/'))
    };
    (allowed(namespace, false) && allowed(path, true)).then(|| format!("{namespace}:{path}"))
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "schedule_command_tests.rs"]
mod command_tests;
