//! Presentation guard: the main thread records the current stage (GPU wait,
//! surface reconfiguration, acquisition, presentation, wait for a draw event);
//! a guard thread logs "presentation blocked: <cause>" when a stage exceeds
//! [`STALL`], then "presentation resumed" when it leaves it. No visible effect: log only.
//! The main thread pays one word written per stage change; the guard, which looks twice a
//! second, measures the time.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Duration beyond which a stage is considered blocked.
pub const STALL: Duration = Duration::from_secs(3);

/// Stage of the frame path tracked by the guard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Stage {
    Idle = 0,
    /// `request_redraw` done, `RedrawRequested` not yet received.
    AwaitRedraw,
    /// CPU-side frame preparation (packets, ticks, meshing).
    Frame,
    /// Bounded wait for the end of the GPU work.
    GpuWait,
    /// Driver call: (re)creation of the swap chain.
    Configure,
    /// Acquisition of the surface image.
    Acquire,
    /// Sending the commands to the GPU.
    Submit,
    /// Presentation of the image.
    Present,
    /// Reading back a capture.
    Readback,
}

impl Stage {
    fn from_u8(v: u8) -> Stage {
        use Stage::*;
        [Idle, AwaitRedraw, Frame, GpuWait, Configure, Acquire, Submit, Present, Readback]
            .get(usize::from(v))
            .copied()
            .unwrap_or(Idle)
    }

    /// Readable cause of the blocked stage.
    pub fn cause(self) -> &'static str {
        match self {
            Stage::Idle => "none",
            Stage::AwaitRedraw => "Windows no longer sends a draw event to the window",
            Stage::Frame => "frame preparation (CPU)",
            Stage::GpuWait => "the GPU does not finish the work sent",
            Stage::Configure => "the driver does not return when recreating the swap chain",
            Stage::Acquire => "the driver does not provide a surface image",
            Stage::Submit => "the driver does not return when the commands are sent",
            Stage::Present => "the driver does not return when presenting",
            Stage::Readback => "capture readback (GPU)",
        }
    }
}

/// Current stage in the low byte, number of stage changes above it: one word, so that the guard
/// reads both together. Written by the main thread only.
static STAGE_AND_CHANGES: AtomicU64 = AtomicU64::new(0);
/// Instant of entry into the stage, in ms since [`origin`] (only without
/// `RUSTCRAFT_OPT_PRESENT_WATCH_SEQUENCE`).
static SINCE: AtomicU64 = AtomicU64::new(0);
/// Win32 handle of the window (log context), 0 = unknown.
static HWND: AtomicIsize = AtomicIsize::new(0);

fn origin() -> Instant {
    static O: OnceLock<Instant> = OnceLock::new();
    *O.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    origin().elapsed().as_millis() as u64
}

/// The main thread only counts its stage changes and the guard measures the time itself
/// (`RUSTCRAFT_OPT_PRESENT_WATCH_SEQUENCE`, `=0`: the main thread stamps every change).
fn guard_measures_time() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_PRESENT_WATCH_SEQUENCE")
}

/// Stage and number of stage changes read together.
fn stage_and_changes() -> (Stage, u64) {
    let word = STAGE_AND_CHANGES.load(Ordering::Acquire);
    (Stage::from_u8(word as u8), word >> 8)
}

fn current_stage() -> Stage {
    stage_and_changes().0
}

/// Enters `stage` (the timer restarts if the stage changes).
pub fn enter(stage: Stage) {
    enter_stamped(stage, !guard_measures_time());
}

/// [`enter`]; `stamp_entry`: the entry instant is read and stored here, on the main thread.
fn enter_stamped(stage: Stage, stamp_entry: bool) {
    let (current, changes) = stage_and_changes();
    if current == stage {
        return;
    }
    // About eight stage changes a frame: without the stamp none of them reads the clock. The
    // change count is enough for the guard to tell a stage held from a stage entered again.
    if stamp_entry {
        SINCE.store(now_ms(), Ordering::Relaxed);
    }
    STAGE_AND_CHANGES.store((changes + 1) << 8 | stage as u64, Ordering::Release);
}

/// End of the current stage.
pub fn leave() {
    enter(Stage::Idle);
}

/// Runs `f` in `stage`, then returns to the previous stage. The frame composition of the perf
/// log charges the call to its own stage (`frame_stages.rs`).
pub fn during<T>(stage: Stage, f: impl FnOnce() -> T) -> T {
    let prev = current_stage();
    enter(stage);
    let r = crate::profiler::during_driver_stage(stage, f);
    enter(prev);
    r
}

/// Tracked window (log context: minimized, hidden).
pub fn set_window(window: &winit::window::Window) {
    #[cfg(windows)]
    {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = window.window_handle()
            && let RawWindowHandle::Win32(w) = h.as_raw()
        {
            HWND.store(w.hwnd.get(), Ordering::Relaxed);
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// Starts the guard thread (once).
pub fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        origin();
        let _ = std::thread::Builder::new().name("presentation guard".into()).spawn(watch);
    });
}

/// Pause of the guard between two looks at the main thread's stage.
const SAMPLE_EVERY: Duration = Duration::from_millis(500);

/// What the guard has to say about a stage.
#[derive(Debug, PartialEq)]
enum GuardReport {
    Blocked { stage: Stage, held: Duration },
    Resumed { stage: Stage, blocked: Duration },
}

fn log(report: GuardReport) {
    match report {
        GuardReport::Blocked { stage, held } => {
            eprintln!("presentation blocked: {} (since {:.1} s; {})", stage.cause(), held.as_secs_f64(), context());
        }
        GuardReport::Resumed { stage, blocked } => {
            eprintln!("presentation resumed: {} ({:.1} s)", stage.cause(), blocked.as_secs_f64());
        }
    }
}

fn watch() {
    if guard_measures_time() { watch_change_count() } else { watch_entry_stamps() }
}

/// Guard reading the entry instant stamped by the main thread at every stage change.
fn watch_entry_stamps() {
    // (stage, entry) reported blocked, waiting for its resumption.
    let mut reported: Option<(Stage, u64)> = None;
    loop {
        std::thread::sleep(SAMPLE_EVERY);
        let stage = current_stage();
        let since = SINCE.load(Ordering::Relaxed);
        if let Some((s, t)) = reported
            && (s != stage || t != since)
        {
            log(GuardReport::Resumed { stage: s, blocked: Duration::from_millis(since.saturating_sub(t)) });
            reported = None;
        }
        if reported.is_none() && stage != Stage::Idle {
            let held = now_ms().saturating_sub(since);
            if held >= STALL.as_millis() as u64 {
                log(GuardReport::Blocked { stage, held: Duration::from_millis(held) });
                reported = Some((stage, since));
            }
        }
    }
}

/// Guard measuring the time itself: the main thread only counts its stage changes.
fn watch_change_count() {
    let mut detector = StallDetector::default();
    loop {
        std::thread::sleep(SAMPLE_EVERY);
        let (stage, changes) = stage_and_changes();
        detector.sample(stage, changes, Instant::now(), &mut log);
    }
}

/// Blocked stage found by sampling. A change count found unchanged from one sample to the next
/// means the stage was held all along: its time is counted from the first sample that saw it, so
/// a block is reported at the same sample as with an entry stamp, and the durations written are
/// within one sampling pause of the real ones.
#[derive(Default)]
struct StallDetector {
    /// Change count of the last sample, and instant of the first sample that saw it.
    seen: Option<(u64, Instant)>,
    /// Stage reported blocked, waiting for its resumption: its change count, when it was first seen.
    reported: Option<(Stage, u64, Instant)>,
}

impl StallDetector {
    fn sample(&mut self, stage: Stage, changes: u64, now: Instant, report: &mut impl FnMut(GuardReport)) {
        let first_seen = match self.seen {
            Some((count, at)) if count == changes => at,
            _ => {
                self.seen = Some((changes, now));
                now
            }
        };
        if let Some((blocked_stage, count, since)) = self.reported
            && count != changes
        {
            report(GuardReport::Resumed { stage: blocked_stage, blocked: now.saturating_duration_since(since) });
            self.reported = None;
        }
        if self.reported.is_none() && stage != Stage::Idle {
            let held = now.saturating_duration_since(first_seen);
            if held >= STALL {
                report(GuardReport::Blocked { stage, held });
                self.reported = Some((stage, changes, first_seen));
            }
        }
    }
}

/// State of the session and the window at the time of the blockage.
fn context() -> String {
    let s = sys::snapshot(HWND.load(Ordering::Relaxed));
    format!(
        "session {}, window {}{}",
        if s.remote { "remote (Remote Desktop)" } else { "local" },
        match (s.visible, s.iconic) {
            (_, true) => "minimized",
            (false, _) => "hidden",
            _ => "visible",
        },
        if s.locked { ", desktop locked or inaccessible" } else { "" }
    )
}

struct Snapshot {
    remote: bool,
    visible: bool,
    iconic: bool,
    locked: bool,
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod sys {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
        fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> isize;
        fn CloseDesktop(desk: isize) -> i32;
    }
    const SM_REMOTESESSION: i32 = 0x1000;
    const DESKTOP_SWITCHDESKTOP: u32 = 0x0100;

    pub(super) fn snapshot(hwnd: isize) -> super::Snapshot {
        // SAFETY: functions with no side effects; `hwnd` is a window handle (or 0, which
        // these functions refuse without error).
        unsafe {
            let desk = OpenInputDesktop(0, 0, DESKTOP_SWITCHDESKTOP);
            if desk != 0 {
                CloseDesktop(desk);
            }
            super::Snapshot {
                remote: GetSystemMetrics(SM_REMOTESESSION) != 0,
                visible: hwnd != 0 && IsWindowVisible(hwnd) != 0,
                iconic: hwnd != 0 && IsIconic(hwnd) != 0,
                locked: desk == 0,
            }
        }
    }
}

#[cfg(not(windows))]
mod sys {
    pub(super) fn snapshot(_: isize) -> super::Snapshot {
        super::Snapshot { remote: false, visible: true, iconic: false, locked: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_round_trips_and_has_cause() {
        for s in [
            Stage::Idle,
            Stage::AwaitRedraw,
            Stage::Frame,
            Stage::GpuWait,
            Stage::Configure,
            Stage::Acquire,
            Stage::Submit,
            Stage::Present,
            Stage::Readback,
        ] {
            assert_eq!(Stage::from_u8(s as u8), s);
            assert!(!s.cause().is_empty());
        }
    }

    /// The only test that moves the stage of the process (the others feed the detector by hand).
    #[test]
    fn during_restores_previous_stage_and_counts_the_changes() {
        enter(Stage::Frame);
        let (_, before) = stage_and_changes();
        let v = during(Stage::Acquire, current_stage);
        assert_eq!(v, Stage::Acquire);
        assert_eq!(stage_and_changes(), (Stage::Frame, before + 2), "in then out: two changes");
        enter(Stage::Frame);
        assert_eq!(stage_and_changes().1, before + 2, "same stage: not a change");
        // The entry instant is stamped by the main thread only when asked for.
        SINCE.store(u64::MAX, Ordering::Relaxed);
        enter_stamped(Stage::Present, false);
        assert_eq!(SINCE.load(Ordering::Relaxed), u64::MAX, "no clock read on a counted change");
        enter_stamped(Stage::Frame, true);
        assert_ne!(SINCE.load(Ordering::Relaxed), u64::MAX);
        leave();
    }

    /// Micro-bench: a stage change stamped by the main thread against a counted one
    /// (`cargo test --release -p rc-app bench_stage_change -- --ignored --nocapture`, alone: it
    /// moves the stage of the process).
    #[test]
    #[ignore = "micro-bench"]
    fn bench_stage_change() {
        const CHANGES: u32 = 4_000_000;
        for (label, stamp_entry) in [("stamped by the main thread", true), ("counted", false)] {
            let start = Instant::now();
            for i in 0..CHANGES {
                enter_stamped(if i % 2 == 0 { Stage::Present } else { Stage::Frame }, stamp_entry);
            }
            println!("stage change {label}: {:.1} ns", start.elapsed().as_nanos() as f64 / f64::from(CHANGES));
        }
        leave();
    }

    /// Feeds the detector one sample every pause, `changes` and `stage` given per sample.
    fn sampled(samples: &[(Stage, u64)]) -> Vec<(usize, GuardReport)> {
        let start = Instant::now();
        let mut detector = StallDetector::default();
        let mut reports = Vec::new();
        for (i, &(stage, changes)) in samples.iter().enumerate() {
            let now = start + SAMPLE_EVERY * (i as u32 + 1);
            detector.sample(stage, changes, now, &mut |r| reports.push((i, r)));
        }
        reports
    }

    /// A presentation held 3 s is reported once, at the sample where the first look at it is 3 s
    /// old (the sample an entry stamp reports it at), then its end with the time it lasted.
    /// Sabotage checked: a threshold one pause longer, or a report repeated at every sample, fail.
    #[test]
    fn a_stage_held_three_seconds_is_reported_once_then_its_end() {
        let mut samples = vec![(Stage::Frame, 6), (Stage::Present, 7)];
        samples.extend([(Stage::Present, 7); 9]); // held: same change count at every look
        samples.extend([(Stage::Idle, 8), (Stage::Idle, 8)]);
        let secs = Duration::from_secs_f64;
        assert_eq!(
            sampled(&samples),
            vec![
                (7, GuardReport::Blocked { stage: Stage::Present, held: secs(3.0) }),
                (11, GuardReport::Resumed { stage: Stage::Present, blocked: secs(5.0) }),
            ]
        );
    }

    /// The same stage found at every look, entered again in between (every frame presents): the
    /// change count moved, nothing is held. Sabotage checked: comparing the stage instead of the
    /// count reports a block.
    #[test]
    fn a_stage_entered_again_between_looks_is_not_held() {
        let samples: Vec<_> = (0..20).map(|i| (Stage::Present, 100 + 8 * i)).collect();
        assert_eq!(sampled(&samples), vec![]);
        // Nothing to say about an idle main thread either, however long.
        assert_eq!(sampled(&[(Stage::Idle, 3); 20]), vec![]);
    }
}
