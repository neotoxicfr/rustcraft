//! Presentation guard: the main thread records the current stage (GPU wait,
//! surface reconfiguration, acquisition, presentation, wait for a draw event);
//! a guard thread logs "presentation blocked: <cause>" when a stage exceeds
//! [`STALL`], then "presentation resumed" when it leaves it. No visible effect: log only.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, AtomicU8, AtomicU64, Ordering};
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

static STAGE: AtomicU8 = AtomicU8::new(0);
/// Instant of entry into the stage, in ms since [`origin`].
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

/// Enters `stage` (the timer restarts if the stage changes).
pub fn enter(stage: Stage) {
    if STAGE.load(Ordering::Relaxed) != stage as u8 {
        SINCE.store(now_ms(), Ordering::Relaxed);
        STAGE.store(stage as u8, Ordering::Release);
    }
}

/// End of the current stage.
pub fn leave() {
    enter(Stage::Idle);
}

/// Runs `f` in `stage`, then returns to the previous stage.
pub fn during<T>(stage: Stage, f: impl FnOnce() -> T) -> T {
    let prev = Stage::from_u8(STAGE.load(Ordering::Relaxed));
    enter(stage);
    let r = f();
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

fn watch() {
    // (stage, entry) reported blocked, waiting for its resumption.
    let mut reported: Option<(Stage, u64)> = None;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let stage = Stage::from_u8(STAGE.load(Ordering::Acquire));
        let since = SINCE.load(Ordering::Relaxed);
        if let Some((s, t)) = reported
            && (s != stage || t != since)
        {
            eprintln!("presentation resumed: {} ({:.1} s)", s.cause(), since.saturating_sub(t) as f64 / 1e3);
            reported = None;
        }
        if reported.is_none() && stage != Stage::Idle {
            let held = now_ms().saturating_sub(since);
            if held >= STALL.as_millis() as u64 {
                eprintln!(
                    "presentation blocked: {} (since {:.1} s; {})",
                    stage.cause(),
                    held as f64 / 1e3,
                    context()
                );
                reported = Some((stage, since));
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

    #[test]
    fn during_restores_previous_stage() {
        enter(Stage::Frame);
        let v = during(Stage::Acquire, || Stage::from_u8(STAGE.load(Ordering::Relaxed)));
        assert_eq!(v, Stage::Acquire);
        assert_eq!(Stage::from_u8(STAGE.load(Ordering::Relaxed)), Stage::Frame);
        leave();
    }
}
