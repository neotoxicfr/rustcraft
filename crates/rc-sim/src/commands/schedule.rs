//! Functions waiting for their tick (`/schedule`). The entries due at one tick run in the order
//! they were scheduled; scheduled events are not saved (game 26.3 keeps them in `level.dat`:
//! separate fidelity point), and an identical `append` is kept twice (the game ignores it).

use std::collections::BTreeMap;

use hashbrown::HashMap;

/// Pending `/schedule` entries: (due tick, function or `#tag` name).
pub struct ScheduledFunctions {
    /// `RUSTCRAFT_OPT_SCHEDULEQUEUE`: entries ordered by (due tick, scheduling order), so a tick
    /// with nothing due reads one key; otherwise the former list, split at every tick.
    ordered: bool,
    list: Vec<(i64, String)>,
    by_due: BTreeMap<(i64, u64), String>,
    /// Keys of each name in `by_due` (`clear` and `replace` remove a name's entries).
    keys_of: HashMap<String, Vec<(i64, u64)>>,
    next_order: u64,
}

impl ScheduledFunctions {
    pub fn new(ordered: bool) -> ScheduledFunctions {
        ScheduledFunctions { ordered, list: Vec::new(), by_due: BTreeMap::new(), keys_of: HashMap::new(), next_order: 0 }
    }

    pub fn len(&self) -> usize {
        if self.ordered { self.by_due.len() } else { self.list.len() }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds an entry after every other one due at the same tick.
    pub fn push(&mut self, due: i64, name: String) {
        if !self.ordered {
            self.list.push((due, name));
            return;
        }
        let key = (due, self.next_order);
        self.next_order += 1;
        self.keys_of.entry_ref(name.as_str()).or_default().push(key);
        self.by_due.insert(key, name);
    }

    /// Removes every entry of `name`; how many there were.
    pub fn remove_named(&mut self, name: &str) -> usize {
        if !self.ordered {
            let before = self.list.len();
            self.list.retain(|(_, n)| n != name);
            return before - self.list.len();
        }
        let Some(keys) = self.keys_of.remove(name) else { return 0 };
        for k in &keys {
            self.by_due.remove(k);
        }
        keys.len()
    }

    /// Takes the entries due at `now` or before, in the order they run: by due tick, then in
    /// scheduling order (every entry is due one tick or more after it was scheduled and ticks
    /// are not skipped, so all of them are due at `now` and the order is the scheduling order,
    /// as before).
    pub fn take_due(&mut self, now: i64) -> Vec<String> {
        if !self.ordered {
            let (due, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.list).into_iter().partition(|(t, _)| *t <= now);
            self.list = rest;
            return due.into_iter().map(|(_, n)| n).collect();
        }
        // Nothing due: one key read, no list split.
        if self.by_due.first_key_value().is_none_or(|((t, _), _)| *t > now) {
            return Vec::new();
        }
        let later = self.by_due.split_off(&(now + 1, 0));
        let due = std::mem::replace(&mut self.by_due, later);
        let mut out = Vec::with_capacity(due.len());
        for (key, name) in due {
            self.forget_key(&name, key);
            out.push(name);
        }
        out
    }

    fn forget_key(&mut self, name: &str, key: (i64, u64)) {
        if let Some(keys) = self.keys_of.get_mut(name) {
            keys.retain(|k| *k != key);
            if keys.is_empty() {
                self.keys_of.remove(name);
            }
        }
    }

    /// Names of the pending entries (suggestions of `schedule clear`).
    pub fn names(&self) -> Vec<String> {
        if self.ordered { self.by_due.values().cloned().collect() } else { self.list.iter().map(|(_, n)| n.clone()).collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*: the same sequence on every platform.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    const NAMES: [&str; 8] = ["a:x", "a:y", "#a:t", "b:z", "a:w", "#b:u", "c:v", "Zoé:é"];

    /// The `/schedule` commands the server runs, applied to one queue.
    fn apply(q: &mut ScheduledFunctions, rng: &mut Rng, now: i64, log: &mut Vec<String>) {
        let base = NAMES[rng.below(NAMES.len() as u64) as usize];
        let name = format!("{base}{}", rng.below(6));
        let due = now + 1 + rng.below(40) as i64;
        match rng.below(10) {
            0..=5 => q.push(due, name), // append
            6..=8 => {
                // replace (the default)
                q.remove_named(&name);
                q.push(due, name);
            }
            _ => log.push(format!("clear {name} -> {}", q.remove_named(&name))),
        }
    }

    /// 10,000 schedules (append, replace, clear, varied ticks, tags) interleaved with 2,000
    /// ticks, the due functions themselves scheduling and clearing: the sequence of functions
    /// launched (and the clear counts) is the same on both queues; `ordered_by` sorts the due
    /// entries of the ordered queue before comparing (sabotage).
    fn trace(ordered: bool, sabotage: bool) -> Vec<String> {
        let mut q = ScheduledFunctions::new(ordered);
        let mut rng = Rng(0x5EED_0F5C);
        let mut log = Vec::new();
        for now in 0..2_000i64 {
            for _ in 0..5 {
                apply(&mut q, &mut rng, now, &mut log);
            }
            let mut due = q.take_due(now);
            if sabotage {
                due.sort();
            }
            for f in due {
                log.push(format!("{now} {f}"));
                // A launched function schedules or clears in turn.
                if rng.below(4) == 0 {
                    apply(&mut q, &mut rng, now, &mut log);
                }
            }
            log.push(format!("pending {}", q.len()));
        }
        log
    }

    #[test]
    fn ordered_queue_launches_like_the_list() {
        let list = trace(false, false);
        let launched = list.iter().filter(|l| !l.starts_with("pending") && !l.starts_with("clear")).count();
        let cleared = list.iter().filter(|l| l.starts_with("clear") && !l.ends_with("-> 0")).count();
        assert!(launched > 1_000 && cleared > 100, "{launched} launched, {cleared} clears that removed something");
        assert_eq!(trace(true, false), list, "ordered queue differs from the list");
        assert_ne!(trace(true, true), list, "sabotage (due entries by name) not detected");
    }

    #[test]
    fn nothing_due_and_names() {
        let mut q = ScheduledFunctions::new(true);
        q.push(10, "a:x".into());
        q.push(5, "a:y".into());
        q.push(10, "a:x".into());
        assert!(q.take_due(4).is_empty());
        assert_eq!(q.take_due(5), ["a:y"]);
        let mut names = q.names();
        names.sort();
        assert_eq!(names, ["a:x", "a:x"]);
        assert_eq!(q.remove_named("a:x"), 2);
        assert!(q.is_empty() && q.keys_of.is_empty());
    }
}
