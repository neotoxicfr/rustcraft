//! Tracking of the output devices: the list is re-read at most once per second off the
//! main thread (by default only after a system notification, or when a cheap probe of the
//! identifiers sees a change); the engine is rebuilt when the current device disappears, when the
//! system default device changes (the "default" setting) or when the chosen device becomes
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

/// Interval between two polls of the devices.
const INTERVAL: Duration = Duration::from_millis(1000);

/// With notifications, full list re-read anyway every this many polls (a notification missed).
const NOTIFIED_FULL_READ: u32 = 30;
/// Without notifications (probing the identifiers), full list re-read every this many polls: a
/// renamed device keeps its identifier.
const PROBED_FULL_READ: u32 = 10;

/// When the poll thread reads the full list again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refresh {
    /// At every poll (old path).
    EveryPoll,
    /// When the identifiers change (`AUDIODEVIDS=1`): a rename that keeps its identifier is
    /// never seen.
    IdsChanged,
    /// `AUDIO_DEVICE_EVENTS` (default): on a system notification (Windows), with a full read
    /// every [`NOTIFIED_FULL_READ`] polls; without notifications (other systems, subscription
    /// refused), when the identifiers change and every [`PROBED_FULL_READ`] polls.
    Events,
}

/// Tracking switches, read once: `AUDIODEVICES` (persistent thread, first poll off the main
/// thread), `AUDIO_DEVICE_EVENTS` (default 1) and `AUDIODEVIDS` (default 0, only read when the
/// former is 0).
fn opt() -> (bool, Refresh) {
    static O: std::sync::OnceLock<(bool, Refresh)> = std::sync::OnceLock::new();
    *O.get_or_init(|| {
        let refresh = if crate::switch("AUDIO_DEVICE_EVENTS", true) {
            Refresh::Events
        } else if crate::switch("AUDIODEVIDS", false) {
            Refresh::IdsChanged
        } else {
            Refresh::EveryPoll
        };
        (crate::switch("AUDIODEVICES", true), refresh)
    })
}

/// System side of the tracking: the real devices, or a scripted system in the proofs.
trait DeviceSource {
    type Ids: PartialEq;
    /// Full list (slow: each name read from the device's property store).
    fn list(&mut self) -> DeviceList;
    /// Identifiers only (cheap).
    fn ids(&mut self) -> Self::Ids;
    /// `Some(true)`: a notification arrived since the last call; `None`: no notifications.
    fn notified(&mut self) -> Option<bool>;
}

/// The system's devices; with `AUDIO_DEVICE_EVENTS` on Windows, subscribed to their notifications
/// (from the poll thread, which keeps the subscription).
struct SystemDevices {
    #[cfg(windows)]
    subscription: Option<crate::device_watch::Subscription>,
}

impl SystemDevices {
    fn new(refresh: Refresh) -> Self {
        #[cfg(windows)]
        {
            let subscription = (refresh == Refresh::Events).then(crate::device_watch::Subscription::new).flatten();
            if refresh == Refresh::Events && subscription.is_none() {
                eprintln!("[audio] device notifications unavailable: identifiers probed every second");
            }
            SystemDevices { subscription }
        }
        #[cfg(not(windows))]
        {
            let _ = refresh;
            SystemDevices {}
        }
    }
}

impl DeviceSource for SystemDevices {
    type Ids = (Option<cpal::DeviceId>, Vec<cpal::DeviceId>);

    fn list(&mut self) -> DeviceList {
        DeviceList::query()
    }

    fn ids(&mut self) -> Self::Ids {
        crate::engine::output_ids()
    }

    #[cfg(windows)]
    fn notified(&mut self) -> Option<bool> {
        self.subscription.as_ref().map(crate::device_watch::Subscription::take)
    }

    #[cfg(not(windows))]
    fn notified(&mut self) -> Option<bool> {
        None
    }
}

/// Decides at each poll whether the full list must be read again.
struct Poller<S: DeviceSource> {
    source: S,
    refresh: Refresh,
    /// Identifiers at the last probe.
    known: Option<S::Ids>,
    /// Polls since the last full read; `None`: no read yet.
    since_full: Option<u32>,
}

impl<S: DeviceSource> Poller<S> {
    fn new(source: S, refresh: Refresh) -> Self {
        Poller { source, refresh, known: None, since_full: None }
    }

    /// One poll: the full list when it had to be read (always at the first poll).
    fn poll(&mut self) -> Option<DeviceList> {
        let since = self.since_full.map_or(u32::MAX, |n| n + 1);
        let read = match self.refresh {
            Refresh::EveryPoll => true,
            Refresh::IdsChanged => self.ids_changed(),
            Refresh::Events => match self.source.notified() {
                Some(notified) => notified || since >= NOTIFIED_FULL_READ,
                // Probe first: the identifiers seen must be the ones of this read.
                None => self.ids_changed() || since >= PROBED_FULL_READ,
            },
        };
        if !read {
            self.since_full = Some(since);
            return None;
        }
        self.since_full = Some(0);
        Some(self.source.list())
    }

    fn ids_changed(&mut self) -> bool {
        let now = self.source.ids();
        if self.known.as_ref() == Some(&now) {
            return false;
        }
        self.known = Some(now);
        true
    }
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
        let (persistent, refresh) = opt();
        // The subscription belongs to the poll thread, never to the caller's (the window's,
        // whose COM apartment must stay single-threaded).
        let refresh = if persistent { refresh } else { Refresh::EveryPoll };
        Self::start(persistent, refresh, move || SystemDevices::new(refresh))
    }

    /// `source` is built on the poll thread (persistent), otherwise read here once.
    fn start<S: DeviceSource>(
        persistent: bool,
        refresh: Refresh,
        source: impl FnOnce() -> S + Send + 'static,
    ) -> Self {
        let mut t = DeviceTracker {
            list: Arc::new(Mutex::new(None)),
            pending: Arc::new(AtomicBool::new(false)),
            last_check: Instant::now(),
            poller: None,
        };
        if !persistent {
            t.store(source().list());
            return t;
        }
        let (list, stop) = (t.list.clone(), Arc::new(AtomicBool::new(false)));
        let flag = stop.clone();
        let spawned = std::thread::Builder::new().name("audio-devices".into()).spawn(move || {
            let mut poller = Poller::new(source(), refresh);
            while !flag.load(Ordering::Acquire) {
                if let Some(l) = poller.poll()
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
            Err(_) => t.store(DeviceList::query()),
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

    /// One fixed device, every read counted.
    struct Fixed;

    impl DeviceSource for Fixed {
        type Ids = ();

        fn list(&mut self) -> DeviceList {
            POLLS.fetch_add(1, Ordering::Relaxed);
            list("A", &["A"])
        }

        fn ids(&mut self) {}

        fn notified(&mut self) -> Option<bool> {
            None
        }
    }

    /// Persistent thread: first poll off the calling thread, `tick` creates no thread, the thread
    /// stops when the tracking is released.
    #[test]
    fn persistent_poller() {
        let mut t = DeviceTracker::start(true, Refresh::EveryPoll, || Fixed);
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
        let t = DeviceTracker::start(false, Refresh::EveryPoll, || Fixed);
        assert!(t.with_current(|l| l.is_some()));
    }

    /// System state: default device identifier, every device as (identifier, name).
    type State = (u32, Vec<(u32, &'static str)>);

    /// Scripted system: its state at each poll (one per second); a notification for every
    /// change except at the polls in `missed`, or none at all when `notify` is false (probing).
    struct Scripted {
        timeline: Vec<State>,
        now: std::rc::Rc<std::cell::Cell<usize>>,
        notified_up_to: usize,
        notify: bool,
        missed: Vec<usize>,
        reads: u32,
    }

    impl DeviceSource for Scripted {
        type Ids = (u32, Vec<u32>);

        fn list(&mut self) -> DeviceList {
            self.reads += 1;
            let (default, all) = &self.timeline[self.now.get()];
            let name = |id: u32| all.iter().find(|d| d.0 == id).map(|d| d.1.to_owned());
            DeviceList { default: name(*default), all: all.iter().map(|d| d.1.to_owned()).collect() }
        }

        fn ids(&mut self) -> (u32, Vec<u32>) {
            let (default, all) = &self.timeline[self.now.get()];
            (*default, all.iter().map(|d| d.0).collect())
        }

        fn notified(&mut self) -> Option<bool> {
            if !self.notify {
                return None;
            }
            let now = self.now.get();
            let changed = (self.notified_up_to + 1..=now)
                .any(|t| self.timeline[t] != self.timeline[t - 1] && !self.missed.contains(&t));
            self.notified_up_to = now;
            Some(changed)
        }
    }

    /// 45 s: speakers and headset; headset renamed at 3 s; USB card plugged at 6 s and made the
    /// default; speakers unplugged at 9 s; USB card renamed at 14 s; headset default at 20 s and
    /// renamed back at 27 s.
    fn timeline() -> Vec<State> {
        let mut out = Vec::new();
        for t in 0..45 {
            let headset = if (3..27).contains(&t) { "Headset (2)" } else { "Headset" };
            let usb = if t < 14 { "USB" } else { "USB DAC" };
            let mut all = Vec::new();
            if t < 9 {
                all.push((1, "Speakers"));
            }
            all.push((2, headset));
            if t >= 6 {
                all.push((3, usb));
            }
            let default = match t {
                0..6 => 1,
                6..20 => 3,
                _ => 2,
            };
            out.push((default, all));
        }
        out
    }

    /// Lists seen by the client at each second (the last list read), and the full reads.
    fn seen(refresh: Refresh, notify: bool, missed: &[usize]) -> (Vec<DeviceList>, u32) {
        let now = std::rc::Rc::new(std::cell::Cell::new(0));
        let source = Scripted {
            timeline: timeline(),
            now: now.clone(),
            notified_up_to: 0,
            notify,
            missed: missed.to_vec(),
            reads: 0,
        };
        let mut poller = Poller::new(source, refresh);
        let mut current = DeviceList::default();
        let mut out = Vec::new();
        for t in 0..45 {
            now.set(t);
            if let Some(l) = poller.poll() {
                current = l;
            }
            out.push(current.clone());
        }
        (out, poller.source.reads)
    }

    /// Changes of the seen list: (second, new list).
    fn changes(seen: &[DeviceList]) -> Vec<(usize, DeviceList)> {
        (0..seen.len()).filter(|&t| t == 0 || seen[t] != seen[t - 1]).map(|t| (t, seen[t].clone())).collect()
    }

    /// Reload decisions of the client over the seen lists: seconds where it reopens.
    fn decisions(seen: &[DeviceList], name: &str, selected: &str) -> Vec<usize> {
        let mut last = None;
        (0..seen.len()).filter(|&t| should_change_seen(name, selected, &seen[t], &mut last)).collect()
    }

    /// At every second, the new view is what the old one showed at most `late` seconds before
    /// (two close changes may then be seen as one), and both end on the same list.
    fn assert_lags_within(old: &[DeviceList], new: &[DeviceList], late: usize, what: &str) {
        for (t, l) in new.iter().enumerate() {
            let ok = (t.saturating_sub(late)..=t).any(|u| old[u] == *l);
            assert!(ok, "{what}: at {t} s, a list older than {late} s or never shown: {l:?}");
        }
        assert_eq!(old.last(), new.last(), "{what}: never caught up");
    }

    /// `AUDIO_DEVICE_EVENTS`: with notifications, the client sees exactly what the full read every
    /// second showed it (renames included) for a few full reads; a missed notification is caught
    /// up by the slow full read; without notifications, the identifier probe sees plugs and
    /// default changes at once and renames within `PROBED_FULL_READ` seconds. The old
    /// identifier-only probe (`AUDIODEVIDS=1`) never sees a rename.
    #[test]
    fn device_events_replay_the_old_decisions() {
        let (old, old_reads) = seen(Refresh::EveryPoll, true, &[]);
        assert_eq!(old_reads, 45);
        assert!(changes(&old).len() >= 7, "timeline barely changes: empty proof");
        let (events, reads) = seen(Refresh::Events, true, &[]);
        assert_eq!(events, old, "notified: same lists at every second");
        assert!(reads <= 10, "{reads} full reads");
        // Same lists, so the same reload decisions, for choices that follow renames.
        for (name, selected) in [("Speakers", ""), ("Headset (2)", "Headset (2)"), ("USB", "USB DAC"), ("Headset", "")] {
            let d = decisions(&old, name, selected);
            assert!(!d.is_empty(), "{name} / {selected}: no decision: empty proof");
            assert_eq!(decisions(&events, name, selected), d, "{name} / {selected}");
        }
        let (missed, _) = seen(Refresh::Events, true, &[14]);
        assert_lags_within(&old, &missed, NOTIFIED_FULL_READ as usize, "missed notification");
        assert_ne!(missed, old, "the missed notification must delay something: empty proof");
        let (probed, probed_reads) = seen(Refresh::Events, false, &[]);
        assert_lags_within(&old, &probed, PROBED_FULL_READ as usize, "probed");
        assert!(probed_reads < old_reads / 2, "{probed_reads} full reads");
        let (ids_only, _) = seen(Refresh::IdsChanged, true, &[]);
        assert!(ids_only[44].all.contains(&"Headset (2)".to_owned()), "identifier probe saw a rename");
    }

    /// Real system (Windows): the subscription is made and released on its own thread, then the
    /// polls read the list once and afterwards only on a notification.
    #[cfg(windows)]
    #[test]
    fn system_subscription_round_trip() {
        let polls = std::thread::spawn(|| {
            let mut poller = Poller::new(SystemDevices::new(Refresh::Events), Refresh::Events);
            let subscribed = poller.source.subscription.is_some();
            let reads: Vec<bool> = (0..3).map(|_| poller.poll().is_some()).collect();
            (subscribed, reads)
        })
        .join()
        .expect("poll thread");
        if !polls.0 {
            eprintln!("no audio endpoint service: subscription not tested");
            return;
        }
        assert!(polls.1[0], "first poll reads the list");
    }

    /// Bench (ignored), loaded machine figures only: cost of one poll of the real devices, per
    /// refresh rule (full read every second, identifier probe, notification flag).
    /// `cargo-file.ps1 test --release -p rc-audio device_poll_bench -- --ignored --nocapture`
    #[test]
    #[ignore = "bench"]
    fn device_poll_bench() {
        for pass in 0..2 {
            for refresh in [Refresh::EveryPoll, Refresh::IdsChanged, Refresh::Events] {
                let mut poller = Poller::new(SystemDevices::new(refresh), refresh);
                poller.poll();
                const N: u32 = 40;
                let t = Instant::now();
                let reads = (0..N).filter(|_| poller.poll().is_some()).count();
                let ms = t.elapsed().as_secs_f64() * 1e3 / f64::from(N);
                println!("pass {pass} {refresh:?}: {ms:.3} ms per poll, {reads} full reads of {N}");
            }
        }
    }
}
