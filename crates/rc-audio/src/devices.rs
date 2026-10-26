//! Tracking of the output devices: the list is re-read at most once per second off the
//! main thread; the engine is rebuilt when the current device disappears, when the system
//! default device changes (the "default" setting) or when the chosen device becomes
//! available again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Known devices: the default one and all of them, in the format of the `soundDevice` setting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceList {
    pub default: Option<String>,
    pub all: Vec<String>,
}

impl DeviceList {
    /// Reading of the system list (slow: device enumeration and reading of each name
    /// from its property store).
    pub fn query() -> Self {
        DeviceList { default: crate::engine::default_device(), all: crate::engine::output_devices() }
    }
}

/// Number of identifiers read by the cheap probe (`AUDIODEVIDS`), for measurements.
#[doc(hidden)]
pub fn output_ids_count() -> usize {
    let (default, all) = crate::engine::output_ids();
    all.len() + usize::from(default.is_some())
}

/// Interval between two re-reads of the list.
const INTERVAL: Duration = Duration::from_millis(1000);

/// Tracking switches, read once: `AUDIODEVICES` (persistent thread, first poll off the
/// main thread) and `AUDIODEVIDS` (default 0: names re-read only when the identifiers
/// change; a renamed device that keeps its identifier would no longer be seen).
fn opt() -> (bool, bool) {
    static O: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    *O.get_or_init(|| (crate::switch("AUDIODEVICES", true), crate::switch("AUDIODEVIDS", false)))
}

/// List kept up to date by periodic background re-reading.
pub struct DeviceTracker {
    /// `None`: first poll not done yet (persistent thread).
    list: Arc<Mutex<Option<DeviceList>>>,
    pending: Arc<AtomicBool>,
    last_check: Instant,
    /// Persistent thread: stop flag and handle (woken to exit without waiting 1 s).
    poller: Option<(Arc<AtomicBool>, std::thread::Thread)>,
}

impl DeviceTracker {
    /// Tracking started: list read now (old path) or by the "audio-devices" thread, which
    /// then re-reads it every second.
    pub fn new() -> Self {
        let (persistent, ids) = opt();
        Self::start(persistent, ids, DeviceList::query)
    }

    fn start(persistent: bool, ids: bool, query: fn() -> DeviceList) -> Self {
        let mut t = DeviceTracker {
            list: Arc::new(Mutex::new(None)),
            pending: Arc::new(AtomicBool::new(false)),
            last_check: Instant::now(),
            poller: None,
        };
        if !persistent {
            t.store(query());
            return t;
        }
        let (list, stop) = (t.list.clone(), Arc::new(AtomicBool::new(false)));
        let flag = stop.clone();
        let spawned = std::thread::Builder::new().name("audio-devices".into()).spawn(move || {
            let mut known: Option<(Option<cpal::DeviceId>, Vec<cpal::DeviceId>)> = None;
            while !flag.load(Ordering::Acquire) {
                let l = if ids {
                    let now = crate::engine::output_ids();
                    if known.as_ref() == Some(&now) {
                        None
                    } else {
                        known = Some(now);
                        Some(query())
                    }
                } else {
                    Some(query())
                };
                if let Some(l) = l
                    && let Ok(mut g) = list.lock()
                    && g.as_ref() != Some(&l)
                {
                    *g = Some(l);
                }
                let until = Instant::now() + INTERVAL;
                while !flag.load(Ordering::Acquire) {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    std::thread::park_timeout(left);
                }
            }
        });
        match spawned {
            Ok(h) => t.poller = Some((stop, h.thread().clone())),
            // No thread: first poll here, no more updates (like a silent enumeration).
            Err(_) => t.store(query()),
        }
        t
    }

    fn store(&self, l: DeviceList) {
        if let Ok(mut g) = self.list.lock() {
            *g = Some(l);
        }
    }

    /// Last list read (empty before the first poll).
    pub fn current(&self) -> DeviceList {
        self.list.lock().ok().and_then(|l| l.clone()).unwrap_or_default()
    }

    /// Last list read, under the lock, without copying; `None` before the first poll.
    pub fn with_current<R>(&self, f: impl FnOnce(Option<&DeviceList>) -> R) -> R {
        match self.list.lock() {
            Ok(g) => f(g.as_ref()),
            Err(_) => f(None),
        }
    }

    /// Immediate re-read, on the calling thread (lost device).
    pub fn force_refresh(&mut self) {
        self.last_check = Instant::now();
        self.store(DeviceList::query());
    }

    /// Restarts a background read if the interval has elapsed and none is running
    /// (old path: one thread per re-read; persistent thread: nothing to do).
    pub fn tick(&mut self) {
        if self.poller.is_some() || self.last_check.elapsed() < INTERVAL {
            return;
        }
        self.last_check = Instant::now();
        if self.pending.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        let (list, pending) = (self.list.clone(), self.pending.clone());
        let spawned = std::thread::Builder::new().name("audio-devices".into()).spawn(move || {
            let l = DeviceList::query();
            if let Ok(mut g) = list.lock() {
                *g = Some(l);
            }
            pending.store(false, Ordering::Release);
        });
        if spawned.is_err() {
            self.pending.store(false, Ordering::Release);
        }
    }
}

impl Drop for DeviceTracker {
    fn drop(&mut self) {
        // Persistent thread notified and woken; it exits by itself (poll in progress finished).
        if let Some((stop, thread)) = self.poller.take() {
            stop.store(true, Ordering::Release);
            thread.unpark();
        }
    }
}

impl Default for DeviceTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// [`should_change`] with first poll: empty `last_seen` = first list seen, kept without
/// deciding anything (like the old tracking, which started from the list read at its creation).
pub fn should_change_seen(
    current_name: &str,
    selected: &str,
    current: &DeviceList,
    last_seen: &mut Option<DeviceList>,
) -> bool {
    match last_seen {
        None => {
            *last_seen = Some(current.clone());
            false
        }
        Some(seen) => should_change(current_name, selected, current, seen),
    }
}

/// Must the engine be rebuilt? `current_name`: opened device; `selected`: setting
/// (empty = default); `last_seen`: list seen at the last call, updated if it changed.
/// Nothing is decided while the list does not change.
pub fn should_change(current_name: &str, selected: &str, current: &DeviceList, last_seen: &mut DeviceList) -> bool {
    if current == last_seen {
        return false;
    }
    let mut change = false;
    if !current.all.iter().any(|d| d == current_name) {
        eprintln!("[audio] the current device has disappeared");
        change = true;
    }
    if selected.is_empty() {
        if current.default.as_deref() != Some(current_name) {
            eprintln!("[audio] the system default device has changed");
            change = true;
        }
    } else if current_name != selected && current.all.iter().any(|d| d == selected) {
        eprintln!("[audio] the chosen device is available again");
        change = true;
    }
    *last_seen = current.clone();
    change
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(default: &str, all: &[&str]) -> DeviceList {
        DeviceList { default: Some(default.into()), all: all.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn device_changes() {
        let mut seen = list("A", &["A", "B"]);
        // List unchanged: nothing.
        assert!(!should_change("A", "", &list("A", &["A", "B"]), &mut seen));
        // Headset plugged in, became the default device.
        assert!(should_change("A", "", &list("C", &["A", "B", "C"]), &mut seen));
        assert_eq!(seen, list("C", &["A", "B", "C"]));
        // Current device unplugged (explicit choice B absent).
        let mut seen = list("A", &["A", "B"]);
        assert!(should_change("B", "B", &list("A", &["A"]), &mut seen));
        // Explicit choice back.
        let mut seen = list("A", &["A"]);
        assert!(should_change("A", "B", &list("A", &["A", "B"]), &mut seen));
        // Explicit choice: a new default does not move the sound.
        let mut seen = list("A", &["A", "B"]);
        assert!(!should_change("B", "B", &list("C", &["A", "B", "C"]), &mut seen));
    }

    /// Same sequence of polls: the old tracking (list read at creation) and the new one (first
    /// poll kept without a decision) make the same decisions; the first poll never
    /// reloads.
    #[test]
    fn first_observation_matches_old_tracker() {
        let seq = [
            list("A", &["A", "B"]),
            list("A", &["A", "B"]),
            list("C", &["A", "B", "C"]),
            list("C", &["A", "B", "C"]),
            list("C", &["B", "C"]),
            list("A", &["A", "B", "C"]),
        ];
        for (name, selected) in [("A", ""), ("B", "B"), ("A", "B"), ("C", "")] {
            let mut old_seen = seq[0].clone();
            let old: Vec<bool> = seq[1..].iter().map(|l| should_change(name, selected, l, &mut old_seen)).collect();
            let mut seen = None;
            let new: Vec<bool> = seq.iter().map(|l| should_change_seen(name, selected, l, &mut seen)).collect();
            assert!(!new[0], "first poll: no reload");
            assert_eq!(old, new[1..], "{name} / {selected}");
        }
        // The sequence contains decisions of both kinds (the proof is not empty).
        let mut seen = None;
        let d: Vec<bool> = seq.iter().map(|l| should_change_seen("A", "", l, &mut seen)).collect();
        assert!(d.contains(&true) && d.contains(&false));
    }

    static POLLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn fake_query() -> DeviceList {
        POLLS.fetch_add(1, Ordering::Relaxed);
        list("A", &["A"])
    }

    /// Persistent thread: first poll off the calling thread, `tick` creates no thread, the thread
    /// stops when the tracking is released.
    #[test]
    fn persistent_poller() {
        let mut t = DeviceTracker::start(true, false, fake_query);
        let start = Instant::now();
        while t.with_current(|l| l.is_none()) {
            assert!(start.elapsed() < Duration::from_secs(5), "first poll never done");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(t.current(), list("A", &["A"]));
        t.last_check = Instant::now() - INTERVAL * 2;
        t.tick();
        assert!(!t.pending.load(Ordering::Relaxed), "tick started a re-read");
        drop(t);
        std::thread::sleep(Duration::from_millis(100));
        let n = POLLS.load(Ordering::Relaxed);
        std::thread::sleep(INTERVAL + Duration::from_millis(300));
        assert_eq!(POLLS.load(Ordering::Relaxed), n, "poll thread still active");
        // Old path: list read right away, on this thread.
        let t = DeviceTracker::start(false, false, fake_query);
        assert!(t.with_current(|l| l.is_some()));
    }
}
