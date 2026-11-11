//! Frame pacing of the event loop: when the next frame may start.
//!
//! The limit is the game's (`video.rs` `FrameLimiter`), and so is the rule of the wait: every
//! frame period lasts a whole number of nanoseconds and is counted from the end of the previous
//! wait. The wait itself is the event loop's, so the input keeps being read meanwhile. With
//! vertical sync a looser cap, ours, only catches a driver that presents without waiting for the
//! screen.
//!
//! The loop turns far more often than a frame starts (every raw mouse event wakes it): what a
//! turn needs is kept between turns (the period in force, the screen's refresh rate), so that a
//! turn that draws nothing costs one clock read and a comparison.

use std::cell::Cell;
use std::time::{Duration, Instant};

use winit::event_loop::{ActiveEventLoop, ControlFlow};

use crate::video::UNLIMITED;

/// The pacing rules, each behind its switch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PacingRules {
    /// A period lasts a whole number of nanoseconds and is counted from the deadline the loop
    /// waited for. Off (`RUSTCRAFT_OPT_EXACT_FRAME_PERIOD=0`): counted from the wake-up, whose
    /// lateness is then added to every frame (60 asked, 58 to 59 shown).
    pub exact_period: bool,
    /// The last stretch before the deadline is spent turning the loop instead of sleeping, like
    /// the game's busy wait (`RUSTCRAFT_OPT_POLL_TO_FRAME_DEADLINE=1`; off by default: it keeps a
    /// core busy for that stretch of every frame).
    pub poll_to_deadline: bool,
    /// The cap that goes with vertical sync sits above the screen's rate. Off
    /// (`RUSTCRAFT_OPT_VSYNC_CAP_ABOVE_REFRESH=0`): at the rate reported, rounded.
    pub cap_above_refresh: bool,
}

impl PacingRules {
    /// Rules in force (each switch read once).
    pub fn in_force() -> PacingRules {
        PacingRules {
            exact_period: rc_data::opt_switch!("RUSTCRAFT_OPT_EXACT_FRAME_PERIOD"),
            poll_to_deadline: rc_data::opt_switch!("RUSTCRAFT_OPT_POLL_TO_FRAME_DEADLINE", default_off),
            cap_above_refresh: rc_data::opt_switch!("RUSTCRAFT_OPT_VSYNC_CAP_ABOVE_REFRESH"),
        }
    }
}

/// Limit in frames per second when there is one.
fn limited(limit: i32) -> Option<u32> {
    (limit > 0 && limit < UNLIMITED).then_some(limit as u32)
}

/// Frame period of a limit: the game's whole nanoseconds, or the former rounded fraction.
fn limit_period(frames_per_second: u32, exact: bool) -> Duration {
    if exact {
        Duration::from_nanos(1_000_000_000 / u64::from(frames_per_second))
    } else {
        Duration::from_secs_f64(1.0 / f64::from(frames_per_second))
    }
}

/// Period of the cap that goes with vertical sync. The screen paces the frames (the presentation
/// waits for it); the cap only catches a driver that does not wait (remote session, hidden
/// window: several thousand frames a second seen). It has to stay above the screen's real rate:
/// at or under it, the timer and the screen take turns at pacing and a refresh is missed now and
/// then. The system reports whole hertz rounded down (59.94 Hz comes as 59), so one hertz is
/// added, then 2 %.
fn vsync_cap_period(refresh_millihertz: u32) -> Duration {
    let cap_millihertz = u64::from(refresh_millihertz / 1000 + 1) * 1020;
    Duration::from_nanos(1_000_000_000_000 / cap_millihertz)
}

/// Shortest time between two frame starts, `None` = no limit. `limit`: the game's limit
/// (`UNLIMITED` = none); `vsync_refresh_millihertz`: the screen's rate when vertical sync is on
/// and the rate is known.
pub fn frame_period(limit: i32, vsync_refresh_millihertz: Option<u32>, rules: PacingRules) -> Option<Duration> {
    if !rules.cap_above_refresh {
        let limit = vsync_refresh_millihertz.map_or(limit, |mhz| limit.min(((mhz + 500) / 1000) as i32));
        return limited(limit).map(|fps| limit_period(fps, rules.exact_period));
    }
    let of_limit = limited(limit).map(|fps| limit_period(fps, rules.exact_period));
    let of_cap = vsync_refresh_millihertz.filter(|&mhz| mhz > 0).map(vsync_cap_period);
    match (of_limit, of_cap) {
        (Some(limit), Some(cap)) => Some(limit.max(cap)),
        (limit, cap) => limit.or(cap),
    }
}

/// What the event loop does until the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pace {
    /// Draw a frame now.
    Draw,
    /// Sleep until this instant or the next event, then ask again.
    SleepUntil(Instant),
    /// Ask again at the next loop turn, without sleeping.
    PollAgain,
}

/// Lateness of a wake-up the game's limiter still takes for the timer's: it smooths the lateness
/// of its sleeps, never plans for more than this, and stops sleeping that much (plus
/// [`UNSLEPT_MARGIN`]) before the deadline.
const LARGEST_PLANNED_LATENESS: Duration = Duration::from_millis(2);
/// A sleep this late or later is a hitch, not the timer: left out of the smoothed lateness.
const HITCH_LATENESS: Duration = Duration::from_millis(25);
/// Share of a new sleep's lateness in the smoothed one.
const LATENESS_SMOOTHING: f64 = 0.1;
/// Stretch before the deadline that is never slept, whatever the smoothed lateness.
const UNSLEPT_MARGIN: Duration = Duration::from_micros(500);

/// The wait between two frames, in the event loop's terms.
pub struct FramePacer {
    /// Instant the period of the next frame is counted from.
    period_start: Instant,
    /// The loop was told to wait for the deadline of the current period.
    waited_for_deadline: bool,
    /// Wake-up asked from the loop and not seen yet (to measure how late the loop wakes).
    sleeping_until: Option<Instant>,
    /// Smoothed lateness of the loop's wake-ups.
    smoothed_lateness: Duration,
    /// Period the lateness was measured under (another one starts the measure again, like the game).
    measured_period: Option<Duration>,
}

impl Default for FramePacer {
    fn default() -> Self {
        FramePacer {
            period_start: Instant::now(),
            waited_for_deadline: false,
            sleeping_until: None,
            smoothed_lateness: Duration::ZERO,
            measured_period: None,
        }
    }
}

impl FramePacer {
    /// Next step of the loop for frames `period` apart (`None`: no limit). `now` is only read
    /// under a limit.
    pub fn next_step(&mut self, period: Option<Duration>, rules: PacingRules, now: impl FnOnce() -> Instant) -> Pace {
        let Some(period) = period else {
            // The period start goes stale meanwhile, like the game's while its limiter is not
            // called: the first frame under a limit again starts at once.
            self.waited_for_deadline = false;
            self.sleeping_until = None;
            return Pace::Draw;
        };
        let now = now();
        if self.measured_period != Some(period) {
            self.measured_period = Some(period);
            self.smoothed_lateness = Duration::ZERO;
        }
        self.measure_wake_up(now);
        let deadline = self.period_start + period;
        if now >= deadline {
            self.period_start = self.next_period_start(deadline, now, rules);
            self.waited_for_deadline = false;
            return Pace::Draw;
        }
        self.waited_for_deadline = true;
        if !rules.poll_to_deadline {
            return Pace::SleepUntil(deadline);
        }
        let unslept = self.smoothed_lateness + UNSLEPT_MARGIN;
        if deadline - now <= unslept {
            return Pace::PollAgain;
        }
        let wake_up = deadline - unslept;
        self.sleeping_until = Some(wake_up);
        Pace::SleepUntil(wake_up)
    }

    /// Where the period after `deadline` starts, the frame being let through at `now`. The game
    /// counts from the end of its wait: the deadline itself when it waited for it (its busy wait
    /// ends within microseconds), the present instant when the frame came in late (no catching
    /// up). A wake-up of the loop late by no more than a timer's lateness is the first case.
    fn next_period_start(&self, deadline: Instant, now: Instant, rules: PacingRules) -> Instant {
        let woke_for_deadline = self.waited_for_deadline && now - deadline <= LARGEST_PLANNED_LATENESS;
        if rules.exact_period && woke_for_deadline { deadline } else { now }
    }

    /// Lateness of the wake-up the loop was asked for, smoothed like the game's sleeps. A loop
    /// woken early by an event measures nothing.
    fn measure_wake_up(&mut self, now: Instant) {
        let Some(asked) = self.sleeping_until.take() else { return };
        let lateness = now.saturating_duration_since(asked);
        if lateness.is_zero() || lateness >= HITCH_LATENESS {
            return;
        }
        let smoothed = LATENESS_SMOOTHING * lateness.as_secs_f64()
            + (1.0 - LATENESS_SMOOTHING) * self.smoothed_lateness.as_secs_f64();
        self.smoothed_lateness = Duration::from_secs_f64(smoothed).min(LARGEST_PLANNED_LATENESS);
    }
}

/// Refresh rate of the window's screen as the system reports it, in millihertz. Asking costs
/// system calls on the display settings and used to be done at every loop turn with vertical sync
/// (and at every frame by the F3 screen): the answer is kept, and asked again after a window
/// event that may change it or once it is a second old.
#[derive(Default)]
pub struct ScreenRefresh {
    /// Last answer and when it was given.
    last_answer: Cell<Option<(Instant, Option<u32>)>>,
}

impl ScreenRefresh {
    /// A video mode changed outside the game sends the window no event: asked again after this long.
    const MAX_AGE: Duration = Duration::from_secs(1);

    pub fn millihertz(&self, window: &winit::window::Window) -> Option<u32> {
        let ask = || window.current_monitor().and_then(|m| m.refresh_rate_millihertz());
        if !rc_data::opt_switch!("RUSTCRAFT_OPT_REFRESH_RATE_CACHE") {
            return ask();
        }
        self.millihertz_at(Instant::now(), ask)
    }

    /// The kept answer while it is fresh at `now`, otherwise a new one from `ask`.
    fn millihertz_at(&self, now: Instant, ask: impl FnOnce() -> Option<u32>) -> Option<u32> {
        if let Some((given_at, rate)) = self.last_answer.get()
            && now.saturating_duration_since(given_at) < Self::MAX_AGE
        {
            return rate;
        }
        let rate = ask();
        self.last_answer.set(Some((now, rate)));
        rate
    }

    /// The window may be on another screen, or its screen in another video mode.
    pub fn forget(&self) {
        self.last_answer.set(None);
    }
}

/// Pacing state of the event loop.
#[derive(Default)]
pub struct FramePacing {
    pacer: FramePacer,
    /// Period in force and the revision of the settings it was worked out at; `None`: to work out.
    kept_period: Option<(u64, Option<Duration>)>,
    pub screen_refresh: ScreenRefresh,
}

impl FramePacing {
    /// Something that sets the frame limit may have changed.
    pub fn forget_period(&mut self) {
        self.kept_period = None;
    }

    /// The period kept for the settings at `options_revision` (its value may be `None`: no
    /// limit); `None`: to work out again. `keep` off (`RUSTCRAFT_OPT_FRAME_DEADLINE_CACHE=0`):
    /// nothing is kept, every loop turn works it out.
    pub fn kept_period(&self, options_revision: u64, keep: bool) -> Option<Option<Duration>> {
        let (revision, period) = self.kept_period?;
        (keep && revision == options_revision).then_some(period)
    }

    pub fn keep_period(&mut self, options_revision: u64, period: Option<Duration>) {
        self.kept_period = Some((options_revision, period));
    }
}

impl crate::App {
    /// Shortest time between two frame starts, `None` = no limit: the frame limit (`maxFps`, 60
    /// outside a world, 10 minimised or in exclusive fullscreen without focus, 30 then 10 after 1
    /// and 10 min without input) and the cap that goes with vertical sync. Automatic captures are
    /// not limited (measurements), except `RUSTCRAFT_FPSLIMIT=1`.
    fn frame_period(&self) -> Option<Duration> {
        let r = self.renderer.as_ref()?;
        let forced = rc_data::env_str!("RUSTCRAFT_FPSLIMIT") == Some("1");
        let limit = if self.capture.is_some() && !forced { UNLIMITED } else { self.frame_limit() };
        let vsync = crate::video::vsync_forced().unwrap_or_else(|| self.options.b("enableVsync"));
        let refresh = if vsync { self.video.pacing.screen_refresh.millihertz(&r.window) } else { None };
        frame_period(limit, refresh, PacingRules::in_force())
    }

    /// After the events: next frame right away, or at the limit's deadline.
    pub(crate) fn pace_frames(&mut self, el: &ActiveEventLoop) {
        if self.renderer.is_none() {
            return;
        }
        let revision = self.options.revision();
        let keep = rc_data::opt_switch!("RUSTCRAFT_OPT_FRAME_DEADLINE_CACHE");
        let period = match self.video.pacing.kept_period(revision, keep) {
            Some(period) => period,
            None => {
                let period = self.frame_period();
                self.video.pacing.keep_period(revision, period);
                period
            }
        };
        match self.video.pacing.pacer.next_step(period, PacingRules::in_force(), Instant::now) {
            Pace::Draw => {
                el.set_control_flow(ControlFlow::Poll);
                if let Some(r) = &self.renderer {
                    r.window.request_redraw();
                }
                crate::present_watch::enter(crate::present_watch::Stage::AwaitRedraw);
            }
            Pace::SleepUntil(wake_up) => el.set_control_flow(ControlFlow::WaitUntil(wake_up)),
            Pace::PollAgain => el.set_control_flow(ControlFlow::Poll),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORMER: PacingRules = PacingRules { exact_period: false, poll_to_deadline: false, cap_above_refresh: false };
    const EXACT: PacingRules = PacingRules { exact_period: true, ..FORMER };
    const EXACT_POLLED: PacingRules = PacingRules { poll_to_deadline: true, ..EXACT };

    fn micros(us: u64) -> Duration {
        Duration::from_micros(us)
    }

    /// Lateness of the loop's wake-ups in the simulated loops: 50 to 600 µs, like the system's
    /// high-resolution timer, the same sequence at every run.
    fn wake_lateness(frame: usize) -> Duration {
        micros(50 + (frame as u64 * 7919) % 551)
    }

    /// Simulated event loop under a limit: `frames` frames of `work` each, the clock moved by hand
    /// (a sleep ends at its target plus the wake lateness, a polled turn lasts 5 µs). Returns the
    /// start of the first period, the instant of every frame start, the period starts counted by
    /// the pacer, and the turns spent polling.
    fn simulate(rules: PacingRules, period: Duration, frames: usize, work: Duration) -> Simulation {
        let origin = Instant::now();
        let mut pacer = FramePacer { period_start: origin, ..Default::default() };
        let mut clock = origin;
        let mut sim = Simulation { origin, frame_starts: Vec::new(), period_starts: Vec::new(), polled_turns: 0 };
        while sim.frame_starts.len() < frames {
            match pacer.next_step(Some(period), rules, || clock) {
                Pace::Draw => {
                    sim.frame_starts.push(clock);
                    sim.period_starts.push(pacer.period_start);
                    clock += work;
                }
                Pace::SleepUntil(target) => clock = target + wake_lateness(sim.frame_starts.len()),
                Pace::PollAgain => {
                    sim.polled_turns += 1;
                    clock += micros(5);
                }
            }
        }
        sim
    }

    struct Simulation {
        origin: Instant,
        frame_starts: Vec<Instant>,
        period_starts: Vec<Instant>,
        polled_turns: usize,
    }

    impl Simulation {
        /// Frames per second over the whole run.
        fn rate(&self) -> f64 {
            let span = *self.frame_starts.last().unwrap() - self.frame_starts[0];
            (self.frame_starts.len() - 1) as f64 / span.as_secs_f64()
        }

        /// Largest distance of a frame start to the grid of deadlines one `period` apart.
        fn worst_distance_to_grid(&self, period: Duration) -> Duration {
            let on_grid = |k: usize| self.origin + period * (k as u32 + 1);
            self.frame_starts.iter().enumerate().map(|(k, &at)| at.saturating_duration_since(on_grid(k))).max().unwrap()
        }
    }

    /// 600 frames asked at 60 a second, 2 ms of work each. The period starts are exactly one game
    /// period apart (16 666 666 ns, required within ±0.1 ms), the frames stay within one wake
    /// lateness of that grid and 60.0 are shown; polled to the deadline they start within one
    /// loop turn of it. The former rule adds the lateness of every wake-up: 58.9 shown. Sabotage
    /// checked: a period counted from the wake-up under the exact rule fails the first assertion.
    #[test]
    fn six_hundred_deadlines_at_sixty_stay_on_the_grid() {
        let period = limit_period(60, true);
        assert_eq!(period, Duration::from_nanos(16_666_666));
        let work = Duration::from_millis(2);

        let exact = simulate(EXACT, period, 600, work);
        for pair in exact.period_starts.windows(2) {
            assert_eq!(pair[1] - pair[0], period, "a period start off the grid");
        }
        assert!(exact.worst_distance_to_grid(period) <= micros(600));
        assert!((exact.rate() - 60.0).abs() < 0.01, "{}", exact.rate());
        assert_eq!(exact.polled_turns, 0);

        let polled = simulate(EXACT_POLLED, period, 600, work);
        assert!(polled.worst_distance_to_grid(period) <= micros(5), "{:?}", polled.worst_distance_to_grid(period));
        assert!((polled.rate() - 60.0).abs() < 0.001, "{}", polled.rate());
        // The price of the precision: every frame turns the loop for the unslept stretch.
        assert!(polled.polled_turns >= 600 * 20, "{}", polled.polled_turns);

        let former = simulate(FORMER, limit_period(60, false), 600, work);
        let drift_per_frame = (former.frame_starts[599] - former.frame_starts[0]).as_secs_f64() / 599.0 - 1.0 / 60.0;
        assert!((drift_per_frame - 325e-6).abs() < 10e-6, "mean wake lateness added to every frame: {drift_per_frame}");
        assert!((former.rate() - 58.85).abs() < 0.05, "{}", former.rate());
    }

    /// The former rules give the former loop, step for step: next frame at `last + 1 / fps`, the
    /// period restarted from the wake-up. Kept as the reference of `RUSTCRAFT_OPT_*=0`. Sabotage
    /// checked: the exact rule applied whatever the rules fails.
    #[test]
    fn former_rules_are_the_former_loop() {
        let period = limit_period(120, false);
        assert_eq!(period, Duration::from_secs_f64(1.0 / 120.0));
        let origin = Instant::now();
        let mut pacer = FramePacer { period_start: origin, ..Default::default() };
        let mut last_frame = origin;
        let mut clock = origin;
        for turn in 0..2_000u64 {
            // Loop turns at uneven instants: events, frames of varied length, sleeps.
            clock += micros(37 + (turn * 2_654_435_761) % 6_000);
            let next = last_frame + period;
            let expected = if clock >= next {
                last_frame = clock;
                Pace::Draw
            } else {
                Pace::SleepUntil(next)
            };
            assert_eq!(pacer.next_step(Some(period), FORMER, || clock), expected, "turn {turn}");
        }
    }

    /// A frame slower than the period restarts the period from the present instant under every
    /// rule (no burst to catch up), and so does a wake-up later than a timer's lateness.
    #[test]
    fn a_late_frame_is_not_caught_up() {
        let period = limit_period(100, true);
        for rules in [FORMER, EXACT, EXACT_POLLED] {
            let origin = Instant::now();
            let mut pacer = FramePacer { period_start: origin, ..Default::default() };
            // The frame took 25 ms: no wait, the next period starts now.
            let late = origin + Duration::from_millis(25);
            assert_eq!(pacer.next_step(Some(period), rules, || late), Pace::Draw);
            assert_eq!(pacer.period_start, late);
            // Waited for the deadline, woken 3 ms after it (a hitch): from now as well.
            assert_ne!(pacer.next_step(Some(period), rules, || late + micros(100)), Pace::Draw);
            let hitch = late + period + Duration::from_millis(3);
            assert_eq!(pacer.next_step(Some(period), rules, || hitch), Pace::Draw);
            assert_eq!(pacer.period_start, hitch, "{rules:?}");
        }
    }

    /// Without a limit a frame starts at every turn and the clock is not read; the first frame
    /// under a limit again starts at once.
    #[test]
    fn no_limit_draws_every_turn_without_the_clock() {
        let mut pacer = FramePacer::default();
        for _ in 0..3 {
            assert_eq!(pacer.next_step(None, EXACT, || unreachable!("no clock read without a limit")), Pace::Draw);
        }
        let later = pacer.period_start + Duration::from_secs(5);
        assert_eq!(pacer.next_step(Some(limit_period(60, true)), EXACT, || later), Pace::Draw);
        assert_eq!(pacer.period_start, later);
    }

    /// Polling to the deadline: the sleep stops short of it by the smoothed wake lateness plus
    /// the margin, the lateness follows the wake-ups (a tenth of each), never above 2 ms, a hitch
    /// left out, and starts again when the limit changes.
    #[test]
    fn the_sleep_stops_short_by_the_measured_lateness() {
        let period = limit_period(50, true);
        let origin = Instant::now();
        let mut pacer = FramePacer { period_start: origin, ..Default::default() };
        let deadline = origin + period;
        // First sleep: nothing measured yet, only the margin is left unslept.
        let first = pacer.next_step(Some(period), EXACT_POLLED, || origin + micros(10));
        assert_eq!(first, Pace::SleepUntil(deadline - UNSLEPT_MARGIN));
        // Woken 400 µs late: within the margin, the loop polls; a tenth of the lateness is kept.
        let woken = deadline - UNSLEPT_MARGIN + micros(400);
        assert_eq!(pacer.next_step(Some(period), EXACT_POLLED, || woken), Pace::PollAgain);
        let kept = pacer.smoothed_lateness;
        assert!(kept.abs_diff(micros(40)) < micros(1), "{kept:?}");
        assert_eq!(pacer.next_step(Some(period), EXACT_POLLED, || deadline), Pace::Draw);
        assert_eq!(pacer.period_start, deadline);
        // Next sleep: shorter by the lateness kept.
        let second = pacer.next_step(Some(period), EXACT_POLLED, || deadline + micros(10));
        assert_eq!(second, Pace::SleepUntil(deadline + period - UNSLEPT_MARGIN - kept));
        // A hitch (30 ms late) is not the timer: the smoothed lateness stays.
        let hitch = deadline + period + Duration::from_millis(30);
        assert_eq!(pacer.next_step(Some(period), EXACT_POLLED, || hitch), Pace::Draw);
        assert_eq!(pacer.smoothed_lateness, kept);
        // Many very late wake-ups: capped.
        for _ in 0..200 {
            pacer.sleeping_until = Some(origin);
            pacer.measure_wake_up(origin + Duration::from_millis(20));
        }
        assert_eq!(pacer.smoothed_lateness, LARGEST_PLANNED_LATENESS);
        // Another limit: measured again from nothing.
        let _ = pacer.next_step(Some(limit_period(30, true)), EXACT_POLLED, || hitch);
        assert_eq!(pacer.smoothed_lateness, Duration::ZERO);
    }

    /// The frame period for every kind of limit, former rules and new ones.
    #[test]
    fn frame_period_of_the_limit_and_the_vsync_cap() {
        let new = PacingRules { cap_above_refresh: true, ..EXACT };
        // No vertical sync: the limit alone; whole nanoseconds or the former fraction.
        assert_eq!(frame_period(UNLIMITED, None, new), None);
        assert_eq!(frame_period(120, None, new), Some(Duration::from_nanos(8_333_333)));
        assert_eq!(frame_period(120, None, FORMER), Some(Duration::from_secs_f64(1.0 / 120.0)));
        assert_eq!(frame_period(0, None, new), None);
        // Former cap: the rounded rate, merged into the limit.
        let former_60 = Some(Duration::from_secs_f64(1.0 / 60.0));
        assert_eq!(frame_period(UNLIMITED, Some(60_000), FORMER), former_60);
        assert_eq!(frame_period(120, Some(59_940), FORMER), former_60);
        assert_eq!(frame_period(30, Some(60_000), FORMER), Some(Duration::from_secs_f64(1.0 / 30.0)));
        // New cap: (whole hertz + 1) × 1.02, so 62.22 a second at 60 Hz; the slower of the two wins.
        let cap_60 = Duration::from_nanos(1_000_000_000_000 / 62_220);
        assert_eq!(frame_period(UNLIMITED, Some(60_000), new), Some(cap_60));
        assert_eq!(frame_period(120, Some(60_000), new), Some(cap_60));
        assert_eq!(frame_period(30, Some(60_000), new), Some(Duration::from_nanos(33_333_333)));
        // Rate unknown (reported 0): no cap, the limit stays.
        assert_eq!(frame_period(120, Some(0), new), Some(Duration::from_nanos(8_333_333)));
        assert_eq!(frame_period(UNLIMITED, Some(0), new), None);
    }

    /// The cap lets the screen pace: its period is shorter than the screen's for every rate the
    /// system may report for it (whole hertz rounded down on Windows, millihertz elsewhere), and
    /// never more than 5 % plus one hertz above. The former cap was at or under the screen's rate
    /// for the same screens. Sabotage checked: the cap at the reported rate fails.
    #[test]
    fn the_vsync_cap_stays_above_the_screen_rate() {
        // (real rate in Hz, millihertz reported)
        let screens = [
            (23.976, 23_000),
            (23.976, 23_976),
            (59.94, 59_000),
            (59.94, 59_940),
            (60.0, 60_000),
            (74.973, 74_000),
            (119.88, 119_000),
            (143.856, 143_000),
            (144.0, 144_000),
            (164.917, 164_000),
            (239.76, 239_000),
            (360.0, 360_000),
        ];
        let mut former_caps_under_the_screen = 0;
        for (real_hz, reported) in screens {
            let cap_hz = 1.0 / vsync_cap_period(reported).as_secs_f64();
            assert!(cap_hz > real_hz * 1.01, "{real_hz} Hz reported {reported}: cap {cap_hz}");
            assert!(cap_hz < real_hz * 1.05 + 1.0, "{real_hz} Hz reported {reported}: cap {cap_hz}");
            let former_hz = f64::from((reported + 500) / 1000);
            former_caps_under_the_screen += usize::from(former_hz <= real_hz);
        }
        assert_eq!(former_caps_under_the_screen, 10, "every screen whose rate is reported in whole hertz");
    }

    /// The screen's rate is asked once, then again only after a second or when forgotten.
    /// Sabotage checked: an answer never aged fails.
    #[test]
    fn the_refresh_rate_is_asked_once_a_second_or_after_a_window_change() {
        let screen = ScreenRefresh::default();
        let asked = Cell::new(0);
        let start = Instant::now();
        let ask_at = |after: Duration, rate: u32| {
            screen.millihertz_at(start + after, || {
                asked.set(asked.get() + 1);
                Some(rate)
            })
        };
        assert_eq!(ask_at(Duration::ZERO, 60_000), Some(60_000));
        for ms in [1, 16, 500, 999] {
            assert_eq!(ask_at(Duration::from_millis(ms), 144_000), Some(60_000), "kept at {ms} ms");
        }
        assert_eq!(asked.get(), 1);
        assert_eq!(ask_at(Duration::from_millis(1_000), 144_000), Some(144_000), "a second old: asked again");
        assert_eq!(asked.get(), 2);
        screen.forget();
        assert_eq!(ask_at(Duration::from_millis(1_001), 59_000), Some(59_000), "forgotten: asked again");
        assert_eq!(asked.get(), 3);
        // An unknown rate is an answer too: not asked at every call.
        screen.forget();
        assert_eq!(screen.millihertz_at(start, || None), None);
        assert_eq!(screen.millihertz_at(start, || unreachable!("kept")), None);
    }

    /// Micro-bench: what a loop turn pays for the pacing, before and after
    /// (`cargo test --release -p rc-app bench_loop_turn_costs -- --ignored --nocapture`).
    #[test]
    #[ignore = "micro-bench"]
    fn bench_loop_turn_costs() {
        use std::hint::black_box;
        const TURNS: u32 = 2_000_000;
        let per_turn_ns = |f: &mut dyn FnMut()| {
            let start = Instant::now();
            for _ in 0..TURNS {
                f();
            }
            start.elapsed().as_nanos() as f64 / f64::from(TURNS)
        };
        // The setting lookups by text key that `video_tick` and the frame limit did at every turn.
        let options = crate::options::Options::default();
        let keys = [
            "fullscreen",
            "exclusiveFullscreen",
            "fullscreenResolution",
            "mipmapLevels",
            "textureFiltering",
            "renderDistance",
            "entityDistanceScaling",
            "maxFps",
            "inactivityFpsLimit",
            "enableVsync",
        ];
        let lookups = per_turn_ns(&mut || {
            for key in keys {
                black_box(options.ty(black_box(key)));
            }
        });
        println!("10 setting lookups by text key: {lookups:.1} ns per turn ({:.1} ns each)", lookups / 10.0);
        // The same turn with the inputs compared.
        let mut video = crate::video::VideoState::default();
        let inputs = crate::video::VideoTickInputs {
            options_revision: options.revision(),
            video_screen_on_top: false,
            server_view: 12,
            renderer_built: true,
        };
        let compared = per_turn_ns(&mut || {
            black_box(video.tick_needed(black_box(inputs), true));
        });
        println!("video values skipped on unchanged inputs: {compared:.1} ns per turn");
        // A turn that only waits for the deadline: kept period, one clock read, one comparison.
        let mut pacing = FramePacing::default();
        pacing.keep_period(1, Some(Duration::from_secs(3_600)));
        let waiting = per_turn_ns(&mut || {
            let period = pacing.kept_period(black_box(1), true).unwrap();
            black_box(pacing.pacer.next_step(period, EXACT, Instant::now));
        });
        println!("turn waiting for the deadline (kept period): {waiting:.1} ns");
        let clock = per_turn_ns(&mut || {
            black_box(Instant::now());
        });
        println!("one clock read: {clock:.1} ns");
    }

    /// Micro-bench: the system's answer on the screen's refresh rate, asked at every loop turn
    /// before (`... bench_refresh_rate_read -- --ignored --nocapture`). Opens no window: an event
    /// loop is enough to reach the screens.
    #[test]
    #[ignore = "micro-bench: needs a desktop session"]
    #[cfg(windows)]
    fn bench_refresh_rate_read() {
        use winit::platform::pump_events::EventLoopExtPumpEvents;
        use winit::platform::windows::EventLoopBuilderExtWindows;
        /// Takes the first screen the event loop knows, at its first turn.
        struct FirstScreen(Option<winit::monitor::MonitorHandle>);
        impl winit::application::ApplicationHandler for FirstScreen {
            fn resumed(&mut self, el: &ActiveEventLoop) {
                self.0 = el.primary_monitor().or_else(|| el.available_monitors().next());
            }
            fn window_event(&mut self, _: &ActiveEventLoop, _: winit::window::WindowId, _: winit::event::WindowEvent) {}
        }
        const READS: u32 = 2_000;
        let mut event_loop = winit::event_loop::EventLoop::builder().with_any_thread(true).build().expect("event loop");
        let mut first_screen = FirstScreen(None);
        let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut first_screen);
        let Some(monitor) = first_screen.0 else {
            println!("no screen in this session");
            return;
        };
        let start = Instant::now();
        let mut rate = None;
        for _ in 0..READS {
            rate = std::hint::black_box(monitor.refresh_rate_millihertz());
        }
        let asked_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(READS);
        let screen = ScreenRefresh::default();
        let start = Instant::now();
        for _ in 0..READS * 100 {
            std::hint::black_box(screen.millihertz_at(Instant::now(), || monitor.refresh_rate_millihertz()));
        }
        let kept_us = start.elapsed().as_secs_f64() * 1e6 / f64::from(READS * 100);
        println!("refresh rate {rate:?} mHz: asked {asked_us:.2} µs per read, kept {kept_us:.3} µs per read");
    }
}
