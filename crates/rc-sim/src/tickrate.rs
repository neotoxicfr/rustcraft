//! Integrated-server tick rate (`tick rate|freeze|step|sprint`) and loop scheduling.
//!
//! The duration of a tick comes from the chosen rate (at least 1 tick per second). Frozen, the world
//! no longer ticks (clocks, weather, blocks, non-player entities, functions), except during the
//! requested steps. In sprint, ticks run back to back without waiting. The loop catches up its lag by
//! chaining ticks; it skips ticks only if the lag exceeds 1 s + 20 ticks, and at
//! most once per 10 s + 100 ticks.

use std::time::{Duration, Instant};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
/// Overload threshold (before the 20 added ticks).
const OVERLOAD_NANOS: u64 = NANOS_PER_SECOND;
/// Minimum interval between two skips (before the 100 added ticks).
const OVERLOAD_INTERVAL_NANOS: u64 = 10 * NANOS_PER_SECOND;

#[derive(Debug, Clone)]
pub struct TickRate {
    rate: f32,
    nanos: u64,
    frozen: bool,
    /// Steps remaining during the freeze (`tick step`).
    step: i32,
    /// The world ticks this tick (recomputed at the start of each tick).
    runs: bool,
    sprint_left: u64,
    sprint_sched: u64,
    sprint_start: Option<Instant>,
    sprint_spent: Duration,
    sprint_was_frozen: bool,
    /// Work durations of the last 100 ticks (ns), in a ring.
    times: Box<[u64; 100]>,
    times_at: usize,
}

impl Default for TickRate {
    fn default() -> Self {
        Self {
            rate: 20.0,
            nanos: NANOS_PER_SECOND / 20,
            frozen: false,
            step: 0,
            runs: true,
            sprint_left: 0,
            sprint_sched: 0,
            sprint_start: None,
            sprint_spent: Duration::ZERO,
            sprint_was_frozen: false,
            times: Box::new([0; 100]),
            times_at: 0,
        }
    }
}

/// End of sprint: ticks per second and milliseconds per tick (text with two decimals).
#[derive(Debug, Clone, PartialEq)]
pub struct SprintReport {
    pub ticks_per_second: i32,
    pub ms_per_tick: String,
}

impl TickRate {
    pub fn rate(&self) -> f32 {
        self.rate
    }

    pub fn nanos_per_tick(&self) -> u64 {
        self.nanos
    }

    pub fn ms_per_tick(&self) -> f32 {
        self.nanos as f32 / 1_000_000.0
    }

    pub fn frozen(&self) -> bool {
        self.frozen
    }

    pub fn step(&self) -> i32 {
        self.step
    }

    pub fn runs(&self) -> bool {
        self.runs
    }

    pub fn sprinting(&self) -> bool {
        self.sprint_sched > 0
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = rate.max(1.0);
        self.nanos = (NANOS_PER_SECOND as f64 / f64::from(self.rate)) as u64;
    }

    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// Start of tick: does the world tick? A requested step is consumed.
    pub fn tick(&mut self) {
        self.runs = !self.frozen || self.step > 0;
        if self.step > 0 {
            self.step -= 1;
        }
    }

    /// `tick step`: only when frozen.
    pub fn step_if_frozen(&mut self, ticks: i32) -> bool {
        if !self.frozen {
            return false;
        }
        self.step = ticks;
        true
    }

    /// `tick step stop`: false if there was no step in progress.
    pub fn stop_stepping(&mut self) -> bool {
        if self.step > 0 {
            self.step = 0;
            true
        } else {
            false
        }
    }

    /// `tick sprint <ticks>`: true if a sprint in progress is interrupted. The freeze is lifted during
    /// the sprint and restored at the end.
    pub fn request_sprint(&mut self, ticks: u64) -> bool {
        let interrupted = self.sprint_left > 0;
        self.sprint_spent = Duration::ZERO;
        self.sprint_sched = ticks;
        self.sprint_left = ticks;
        self.sprint_was_frozen = self.frozen;
        self.frozen = false;
        interrupted
    }

    /// `tick sprint stop`: the report, if ticks were left to sprint.
    pub fn stop_sprinting(&mut self) -> Option<SprintReport> {
        (self.sprint_left > 0).then(|| self.finish_sprint())
    }

    fn finish_sprint(&mut self) -> SprintReport {
        let done = self.sprint_sched - self.sprint_left;
        let ms = (self.sprint_spent.as_nanos() as f64).max(1.0) / 1_000_000.0;
        let ticks_per_second = (1000.0 * done as f64 / ms) as i32;
        let per = if done == 0 { f64::from(self.ms_per_tick()) } else { ms / done as f64 };
        self.sprint_sched = 0;
        self.sprint_spent = Duration::ZERO;
        self.sprint_left = 0;
        self.frozen = self.sprint_was_frozen;
        SprintReport { ticks_per_second, ms_per_tick: format!("{per:.2}") }
    }

    /// Is this tick sprinted? Consumes a tick of the sprint; when exhausted, the sprint ends and its
    /// report is returned.
    pub fn sprint_this_tick(&mut self, report: &mut Option<SprintReport>) -> bool {
        if !self.runs {
            false
        } else if self.sprint_left > 0 {
            self.sprint_start = Some(Instant::now());
            self.sprint_left -= 1;
            true
        } else {
            *report = Some(self.finish_sprint());
            false
        }
    }

    /// Work duration of a tick (for `tick query`).
    pub fn record_tick_time(&mut self, d: Duration) {
        self.times[self.times_at] = d.as_nanos() as u64;
        self.times_at = (self.times_at + 1) % self.times.len();
    }

    /// Mean duration of a tick over the last 100 (ns).
    pub fn average_tick_nanos(&self) -> u64 {
        self.times.iter().sum::<u64>() / self.times.len() as u64
    }

    /// Median, 95th and 99th percentiles of the last 100 ticks (ns), and the sample count.
    pub fn percentiles(&self) -> (u64, u64, u64, usize) {
        let mut v = *self.times;
        v.sort_unstable();
        let n = v.len();
        (v[n / 2], v[(n as f64 * 0.95) as usize], v[(n as f64 * 0.99) as usize], n)
    }

    /// End of the work of a sprinted tick (duration counted for the report).
    pub fn end_tick_work(&mut self) {
        if let Some(t) = self.sprint_start.take() {
            self.sprint_spent += t.elapsed();
        }
    }
}

/// Loop clock: deadline of the next tick and date of the last tick skip.
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    pub next: Instant,
    /// No skip yet: the first one is allowed right away.
    pub last_overload: Option<Instant>,
}

impl Schedule {
    pub fn new(now: Instant) -> Self {
        Self { next: now, last_overload: None }
    }

    /// Prepares a normal tick of `tick_nanos` at `now`: skips ticks only in case of a
    /// big overload (and no more than once per interval), then advances the deadline. Returns the
    /// number of ticks skipped.
    pub fn plan(&mut self, now: Instant, tick_nanos: u64) -> u64 {
        let tick = tick_nanos.max(1);
        let behind = now.saturating_duration_since(self.next).as_nanos() as u64;
        let interval_ok = self.last_overload.is_none_or(|l| {
            self.next.saturating_duration_since(l).as_nanos() as u64 >= OVERLOAD_INTERVAL_NANOS + 100 * tick
        });
        let mut skipped = 0;
        if behind > OVERLOAD_NANOS + 20 * tick && interval_ok {
            skipped = behind / tick;
            self.next += Duration::from_nanos(skipped * tick);
            self.last_overload = Some(self.next);
        }
        self.next += Duration::from_nanos(tick);
        skipped
    }

    /// Sprinted tick at `now`: no waiting, the clock restarts from now.
    pub fn sprint(&mut self, now: Instant) {
        self.next = now;
        self.last_overload = Some(now);
    }

    /// Wait before the next tick (nothing if late).
    pub fn wait(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 50_000_000;

    #[test]
    fn freeze_and_steps() {
        let mut r = TickRate::default();
        r.tick();
        assert!(r.runs());
        assert!(!r.step_if_frozen(3), "step outside freeze");
        r.set_frozen(true);
        r.tick();
        assert!(!r.runs());
        assert!(r.step_if_frozen(2));
        let runs: Vec<bool> = (0..4)
            .map(|_| {
                r.tick();
                r.runs()
            })
            .collect();
        assert_eq!(runs, [true, true, false, false]);
        assert!(!r.stop_stepping());
    }

    #[test]
    fn rate_sets_tick_length() {
        let mut r = TickRate::default();
        r.set_rate(5.0);
        assert_eq!(r.nanos_per_tick(), 200_000_000);
        r.set_rate(0.1);
        assert_eq!(r.rate(), 1.0);
    }

    #[test]
    fn sprint_lifts_freeze_then_restores_it() {
        let mut r = TickRate::default();
        r.set_frozen(true);
        assert!(!r.request_sprint(2));
        assert!(!r.frozen() && r.sprinting());
        let mut rep = None;
        r.tick();
        assert!(r.sprint_this_tick(&mut rep));
        r.end_tick_work();
        r.tick();
        assert!(r.sprint_this_tick(&mut rep));
        r.tick();
        assert!(!r.sprint_this_tick(&mut rep));
        assert!(rep.is_some());
        assert!(r.frozen() && !r.sprinting());
    }

    /// A 1.5 s hiccup is caught up entirely (no tick skipped).
    #[test]
    fn small_lag_is_caught_up() {
        let t0 = Instant::now();
        let mut s = Schedule::new(t0);
        let now = t0 + Duration::from_millis(1500);
        assert_eq!(s.plan(now, T), 0);
        // The following ticks go out without waiting until caught up.
        assert_eq!(s.wait(now), Duration::ZERO);
        let mut n = 1;
        while s.wait(now) == Duration::ZERO {
            s.plan(now, T);
            n += 1;
        }
        assert_eq!(n, 31, "30 ticks due + the current one");
    }

    /// Beyond 2 s of lag, ticks are skipped, but not twice in less than 15 s.
    #[test]
    fn big_lag_skips_at_most_once_per_interval() {
        let t0 = Instant::now();
        let mut s = Schedule::new(t0);
        // Last skip long ago (clock already advanced).
        s.last_overload = Some(t0);
        s.next = t0 + Duration::from_secs(20);
        let now = s.next + Duration::from_secs(3);
        assert_eq!(s.plan(now, T), 60);
        assert_eq!(s.wait(now), Duration::from_nanos(T));
        // Second big lag 5 s later: caught up, not skipped.
        let now2 = now + Duration::from_secs(5);
        assert_eq!(s.plan(now2, T), 0);
    }
}
