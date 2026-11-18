//! Composition of each frame for the perf log (`RUSTCRAFT_PERFLOG_STAGES=1` with
//! `RUSTCRAFT_PERFLOG`): a diagnostic, never on in the reference bench. A collection of the frame
//! profiler (`profiler.rs`) apart from its three text collections, that keeps only the zones right
//! under `root`, by index (no path text), plus `fpsUpdate` (the end of the frame, a child of
//! `frame` in the zone tree), plus the driver stages of the presentation guard (`present_watch.rs`:
//! GPU wait, acquisition, submission, presentation...) which take over from the zone while they
//! last, and the time between two frames (event loop, outside any frame). One clock, always charged
//! to the current stage: the stages of a frame add up exactly to its interval in the perf log,
//! since both are read at the same instant. Only frames in a world are composed: out of a world the
//! whole menu frame runs inside `scheduledExecutables`. Off: the profiler's usual thread boolean,
//! nothing else.

use std::time::Instant;

use crate::present_watch::Stage as DriverStage;

/// Where the main thread is, for the frame's composition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FrameStage {
    /// Between the end of a frame and the start of the next one (event loop, input, idle).
    OutsideFrame,
    /// Inside the frame, outside any top zone.
    Unzoned,
    /// Server packets received (zone `scheduledPacketProcessing`).
    Packets,
    /// Light then meshing scheduled after the packets (zone `scheduledExecutables`; out of a
    /// world the whole menu frame runs in it, so those frames are not composed).
    LightAndMeshing,
    Mouse,
    /// Client ticks.
    Tick,
    Sound,
    /// Frame preparation and drawing on the CPU, driver stages and `fpsUpdate` excluded (zone
    /// `frame`).
    Frame,
    /// End of the frame (zone `fpsUpdate` under `frame`): frame counter, statistics of each
    /// second, and the perf log's own work after its reading, so it shows in the NEXT frame.
    FpsUpdate,
    /// A top zone of another name.
    OtherZone,
    /// Device polled after the submission (finished work picked up, its resources freed).
    GpuWait,
    /// Swap chain (re)created.
    Configure,
    /// Surface image acquired (waits for a free image of the swap chain).
    Acquire,
    Submit,
    Present,
    /// Capture read back.
    Readback,
}

pub const STAGE_COUNT: usize = 16;

impl FrameStage {
    pub const ALL: [FrameStage; STAGE_COUNT] = [
        FrameStage::OutsideFrame,
        FrameStage::Unzoned,
        FrameStage::Packets,
        FrameStage::LightAndMeshing,
        FrameStage::Mouse,
        FrameStage::Tick,
        FrameStage::Sound,
        FrameStage::Frame,
        FrameStage::FpsUpdate,
        FrameStage::OtherZone,
        FrameStage::GpuWait,
        FrameStage::Configure,
        FrameStage::Acquire,
        FrameStage::Submit,
        FrameStage::Present,
        FrameStage::Readback,
    ];

    pub fn name(self) -> &'static str {
        match self {
            FrameStage::OutsideFrame => "outside_frame",
            FrameStage::Unzoned => "unzoned",
            FrameStage::Packets => "packets",
            FrameStage::LightAndMeshing => "light_and_meshing",
            FrameStage::Mouse => "mouse",
            FrameStage::Tick => "tick",
            FrameStage::Sound => "sound",
            FrameStage::Frame => "frame",
            FrameStage::FpsUpdate => "fps_update",
            FrameStage::OtherZone => "other_zone",
            FrameStage::GpuWait => "gpu_wait",
            FrameStage::Configure => "configure",
            FrameStage::Acquire => "acquire",
            FrameStage::Submit => "submit",
            FrameStage::Present => "present",
            FrameStage::Readback => "readback",
        }
    }

    /// Stage of a zone opened right under `root`.
    fn of_top_zone(name: &str) -> FrameStage {
        match name {
            "scheduledPacketProcessing" => FrameStage::Packets,
            "scheduledExecutables" => FrameStage::LightAndMeshing,
            "mouse" => FrameStage::Mouse,
            "tick" => FrameStage::Tick,
            "sound" => FrameStage::Sound,
            "frame" => FrameStage::Frame,
            _ => FrameStage::OtherZone,
        }
    }

    /// Stage of a zone opened right under the top zone `top`: only the end of the frame has one.
    /// The zone tree stays the game's (the F3 chart shows it), so `fpsUpdate` is found at its
    /// own depth rather than moved to the top.
    fn of_child_zone(top: FrameStage, name: &str) -> Option<FrameStage> {
        (top == FrameStage::Frame && name == "fpsUpdate").then_some(FrameStage::FpsUpdate)
    }

    /// Stage of a driver call watched by the presentation guard; `None` for its other states.
    pub fn of_driver(stage: DriverStage) -> Option<FrameStage> {
        match stage {
            DriverStage::GpuWait => Some(FrameStage::GpuWait),
            DriverStage::Configure => Some(FrameStage::Configure),
            DriverStage::Acquire => Some(FrameStage::Acquire),
            DriverStage::Submit => Some(FrameStage::Submit),
            DriverStage::Present => Some(FrameStage::Present),
            DriverStage::Readback => Some(FrameStage::Readback),
            DriverStage::Idle | DriverStage::AwaitRedraw | DriverStage::Frame => None,
        }
    }
}

/// Composition wanted: perf log on and `RUSTCRAFT_PERFLOG_STAGES=1` (read once).
pub fn enabled() -> bool {
    crate::perf_log::on() && rc_data::opt_switch!("RUSTCRAFT_PERFLOG_STAGES", default_off)
}

/// Time of each stage since the last frame's reading (ns).
pub type StageTimes = [u64; STAGE_COUNT];

/// The clock of the main thread's stages, moved by the profiler's zone calls.
pub struct FrameStageClock {
    current: FrameStage,
    /// Top zone (or `Unzoned`) to return to when a driver stage ends.
    zone: FrameStage,
    /// Zones open, `root` included (the length of a profiler collection's stack).
    pub depth: usize,
    since: Instant,
    ns: StageTimes,
}

impl FrameStageClock {
    pub fn new(now: Instant) -> Self {
        FrameStageClock {
            current: FrameStage::OutsideFrame,
            zone: FrameStage::OutsideFrame,
            depth: 0,
            since: now,
            ns: [0; STAGE_COUNT],
        }
    }

    /// Charges the time since the last switch to the current stage, then moves to `to`.
    fn switch(&mut self, to: FrameStage, now: Instant) {
        self.ns[self.current as usize] += now.saturating_duration_since(self.since).as_nanos() as u64;
        self.since = now;
        self.current = to;
    }

    /// Back to the zone, unless a driver stage is running (it ends on the zone it started in).
    fn enter_zone(&mut self, zone: FrameStage, now: Instant) {
        let in_driver = self.current != self.zone;
        self.zone = zone;
        if !in_driver {
            self.switch(zone, now);
        }
    }

    pub fn start_frame(&mut self, now: Instant) {
        self.depth = 1;
        self.enter_zone(FrameStage::Unzoned, now);
    }

    pub fn end_frame(&mut self, now: Instant) {
        self.depth = 0;
        self.enter_zone(FrameStage::OutsideFrame, now);
    }

    /// Zone `name` opened; `now` is only read when it changes the stage (a top zone, or the end
    /// of the frame): the clock stays still for the other nested zones, which are most zone calls.
    pub fn push(&mut self, name: &str, now: impl FnOnce() -> Instant) {
        if self.depth == 0 {
            return; // outside a frame, like the profiler's collections
        }
        self.depth += 1;
        match self.depth {
            2 => self.enter_zone(FrameStage::of_top_zone(name), now()),
            3 => {
                if let Some(stage) = FrameStage::of_child_zone(self.zone, name) {
                    self.enter_zone(stage, now());
                }
            }
            _ => {}
        }
    }

    pub fn pop(&mut self, now: impl FnOnce() -> Instant) {
        match self.depth {
            // The end of the frame closed: back to its top zone.
            3 if self.zone == FrameStage::FpsUpdate => self.enter_zone(FrameStage::Frame, now()),
            2 => self.enter_zone(FrameStage::Unzoned, now()),
            // `root` closed: the frame is over for the collections.
            1 => self.enter_zone(FrameStage::OutsideFrame, now()),
            _ => {}
        }
        self.depth = self.depth.saturating_sub(1);
    }

    /// Start of a driver call: its stage takes over until [`Self::end_driver`].
    pub fn begin_driver(&mut self, stage: FrameStage, now: Instant) {
        self.switch(stage, now);
    }

    /// End of a driver call: back to the zone open now (it may have changed meanwhile).
    pub fn end_driver(&mut self, now: Instant) {
        self.switch(self.zone, now);
    }

    /// Times since the last reading, up to `now`; the clock goes on from `now`.
    pub fn take(&mut self, now: Instant) -> StageTimes {
        let current = self.current;
        self.switch(current, now);
        std::mem::replace(&mut self.ns, [0; STAGE_COUNT])
    }
}

/// One frame of the perf log with its composition.
#[derive(Clone, Copy, Debug)]
pub struct FrameComposition {
    /// Frame interval (ms), the perf log's.
    pub ms: f32,
    pub stage_ns: StageTimes,
    /// Relights of the client during the frame (inside `light_and_meshing`).
    pub relight_ns: u64,
}

/// Readings of a second of frames: means of every stage over all the frames and over the slow
/// ones (interval over twice the median), the slowest frame's own composition.
pub struct SecondComposition {
    pub frame_us: [f64; STAGE_COUNT],
    pub relight_us: f64,
    pub slow_frames: usize,
    pub slow_frame_us: [f64; STAGE_COUNT],
    pub slow_relight_us: f64,
    pub slowest: Option<FrameComposition>,
}

fn mean_us(frames: &[&FrameComposition]) -> ([f64; STAGE_COUNT], f64) {
    let n = frames.len().max(1) as f64;
    let stages = std::array::from_fn(|k| frames.iter().map(|f| f.stage_ns[k]).sum::<u64>() as f64 / 1e3 / n);
    (stages, frames.iter().map(|f| f.relight_ns).sum::<u64>() as f64 / 1e3 / n)
}

impl SecondComposition {
    pub fn of(frames: &[FrameComposition], median_ms: f32) -> SecondComposition {
        let all: Vec<&FrameComposition> = frames.iter().collect();
        let slow: Vec<&FrameComposition> = frames.iter().filter(|f| f.ms > 2.0 * median_ms).collect();
        let (frame_us, relight_us) = mean_us(&all);
        let (slow_frame_us, slow_relight_us) = mean_us(&slow);
        SecondComposition {
            frame_us,
            relight_us,
            slow_frames: slow.len(),
            slow_frame_us,
            slow_relight_us,
            slowest: frames.iter().max_by(|a, b| a.ms.total_cmp(&b.ms)).copied(),
        }
    }
}

/// `12.345 ms = present 8.000 + frame 3.000 + ..., client relight 1.000`: the stages that took
/// time, slowest first.
pub fn composition_text(f: &FrameComposition) -> String {
    let mut stages: Vec<(FrameStage, u64)> =
        FrameStage::ALL.iter().map(|&s| (s, f.stage_ns[s as usize])).filter(|&(_, ns)| ns >= 1_000).collect();
    stages.sort_by_key(|&(_, ns)| std::cmp::Reverse(ns));
    let parts: Vec<String> = stages.iter().map(|&(s, ns)| format!("{} {:.3}", s.name(), ns as f64 / 1e6)).collect();
    format!("{:.3} ms = {}, client relight {:.3}", f.ms, parts.join(" + "), f.relight_ns as f64 / 1e6)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(start: Instant, us: u64) -> Instant {
        start + Duration::from_micros(us)
    }

    /// Every instant of a frame charged to the stage it was in: top zones, nested zones kept in
    /// their top zone, a driver stage inside a zone, the gap between frames. The stages add up to
    /// the interval between the two readings, to the nanosecond.
    #[test]
    fn stages_cover_the_frame_interval() {
        let t0 = Instant::now();
        let mut c = FrameStageClock::new(t0);
        let _ = c.take(t0);
        c.start_frame(at(t0, 100)); // 100 µs outside the frame; `root` open
        c.push("scheduledPacketProcessing", || at(t0, 150)); // 50 µs unzoned
        c.push("nested", || unreachable!("no clock read for a nested zone"));
        c.pop(|| unreachable!("no clock read for a nested zone")); // still packets
        c.pop(|| at(t0, 400)); // packets 250 µs
        c.push("frame", || at(t0, 400));
        c.begin_driver(FrameStage::Present, at(t0, 600)); // frame 200 µs
        c.end_driver(at(t0, 1_600)); // present 1000 µs
        c.pop(|| at(t0, 1_700)); // frame 100 µs more
        c.end_frame(at(t0, 1_750)); // unzoned 50 µs more
        let ns = c.take(at(t0, 2_000)); // 250 µs outside again
        let us = |s: FrameStage| ns[s as usize] / 1_000;
        assert_eq!(us(FrameStage::OutsideFrame), 350);
        assert_eq!(us(FrameStage::Unzoned), 100);
        assert_eq!(us(FrameStage::Packets), 250);
        assert_eq!(us(FrameStage::Frame), 300);
        assert_eq!(us(FrameStage::Present), 1_000);
        assert_eq!(ns.iter().sum::<u64>(), 2_000_000, "the stages cover the interval exactly");
        // The clock goes on from the reading: the next one starts at 2 000 µs.
        assert_eq!(c.take(at(t0, 2_500))[FrameStage::OutsideFrame as usize], 500_000);
    }

    /// A zone closed while a driver stage runs: the driver keeps the time until it ends, then the
    /// clock goes to the zone open at that moment, not to the one the driver started in.
    #[test]
    fn a_zone_switch_during_a_driver_stage_waits_for_its_end() {
        let t0 = Instant::now();
        let mut c = FrameStageClock::new(t0);
        c.start_frame(t0);
        c.push("frame", || t0);
        c.begin_driver(FrameStage::Acquire, at(t0, 10));
        c.pop(|| at(t0, 20));
        c.push("sound", || at(t0, 30));
        assert_eq!(c.current, FrameStage::Acquire, "the driver stage keeps running");
        c.end_driver(at(t0, 50));
        assert_eq!(c.current, FrameStage::Sound);
        let ns = c.take(at(t0, 60));
        assert_eq!(ns[FrameStage::Acquire as usize], 40_000);
        assert_eq!(ns[FrameStage::Frame as usize], 10_000);
        assert_eq!(ns[FrameStage::Sound as usize], 10_000);
        assert_eq!(ns.iter().sum::<u64>(), 60_000);
    }

    /// The end of the frame is a child of `frame` (the zone tree of `app_frame.rs`): it gets its own
    /// stage there, and only there; closed, the clock goes back to `frame`.
    #[test]
    fn end_of_frame_is_found_under_the_frame_zone() {
        let t0 = Instant::now();
        let mut c = FrameStageClock::new(t0);
        c.start_frame(t0);
        c.push("frame", || t0);
        c.push("render", || unreachable!("no clock read for a nested zone"));
        c.pop(|| unreachable!("no clock read for a nested zone"));
        c.push("fpsUpdate", || at(t0, 10)); // frame 10 µs
        c.pop(|| at(t0, 30)); // end of the frame 20 µs
        assert_eq!(c.current, FrameStage::Frame);
        c.pop(|| at(t0, 35)); // frame 5 µs more
        c.push("tick", || at(t0, 35));
        c.push("fpsUpdate", || unreachable!("not the end of the frame under another zone"));
        let ns = c.take(at(t0, 40));
        assert_eq!(ns[FrameStage::Frame as usize], 15_000);
        assert_eq!(ns[FrameStage::FpsUpdate as usize], 20_000);
        assert_eq!(ns[FrameStage::Tick as usize], 5_000);
    }

    /// Slow frames are those over twice the median; the slowest frame is reported whole.
    #[test]
    fn second_composition_keeps_slow_frames_apart() {
        let frame = |ms: f32, frame_us: u64, present_us: u64| {
            let mut stage_ns = [0; STAGE_COUNT];
            stage_ns[FrameStage::Frame as usize] = frame_us * 1_000;
            stage_ns[FrameStage::Present as usize] = present_us * 1_000;
            FrameComposition { ms, stage_ns, relight_ns: 0 }
        };
        let frames = [frame(0.2, 150, 50), frame(0.2, 150, 50), frame(0.5, 100, 400), frame(1.0, 900, 100)];
        let s = SecondComposition::of(&frames, 0.2);
        assert_eq!(s.slow_frames, 2);
        assert_eq!(s.slow_frame_us[FrameStage::Present as usize], 250.0);
        assert_eq!(s.slow_frame_us[FrameStage::Frame as usize], 500.0);
        assert_eq!(s.frame_us[FrameStage::Frame as usize], 325.0);
        assert_eq!(s.slowest.map(|f| f.ms), Some(1.0));
        assert_eq!(composition_text(&frames[3]), "1.000 ms = frame 0.900 + present 0.100, client relight 0.000");
    }
}
