//! Composition of the slowest ticks of the integrated server (`RUSTCRAFT_PERFLOG_STAGES=1`): a
//! diagnostic, never on in the reference bench (`docs/perf.md`). Each tick is cut into its stages
//! by a lap clock (one clock read per stage boundary). Over every window of 100 ticks, the slowest
//! tick is written with its OWN composition and the three ticks after it: never the sum of the
//! per-stage maxima, which no real tick ever had. Its number modulo 400 and 20 shows the periodic
//! work (flowing water every 5 ticks, time sent every 20...). The light time is the server
//! thread's own relights and chunk lighting during the tick: it lies inside the stages (mostly the
//! block-change flush), so it is shown apart and never added to their sum.
//!
//! Lines dated like the client's (`[perf] [t s]`, origin of `generation_timing`): generic counters
//! `server_slowest_tick_*` (column `c:<name>` of `tools/bench/scenes.py`: the mean over the scene
//! window of each window's slowest tick, every column taken from the same ticks so they add up),
//! and one readable line per window. While the switch is on, these dated server lines also fill
//! the client's silences, so the bench no longer sees a client stall: another reason to keep it
//! out of the reference runs. Off: one `Option` test per stage boundary.

use std::time::{Duration, Instant};

/// Stages of a server tick. Stages run in the loop's order; a stage met twice in a tick (chunk
/// adoption, save) adds its two spans.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TickStage {
    /// Chunks read by the loader thread and adopted (start of the tick, then the late batches).
    ChunkAdoption,
    /// Client packets, LAN requests, settings, debug reports.
    Packets,
    /// Save on entering the pause, automatic save.
    Save,
    /// Functions, clocks, border, weather, sleep, scheduled functions.
    WorldClock,
    /// Scheduled block ticks, raids, spawns, random ticks, block events, dragon fight.
    Blocks,
    /// Every entity of the world (and the teleport sent again).
    Entities,
    BlockEntities,
    /// Maps, compasses, dimensions, commands, statistics, game tests.
    WorldAfter,
    /// Changed blocks sent with the light they need, then the action acknowledgement.
    BlockChangeFlush,
    /// The LAN guests' own tick.
    LanGuests,
    /// Tick of a paused world.
    Paused,
    /// View tracking and generation results.
    Tracking,
    ChunkSend,
    /// Progress and map of the loading screen.
    LoadingScreen,
}

const STAGE_COUNT: usize = 14;

impl TickStage {
    const ALL: [TickStage; STAGE_COUNT] = [
        TickStage::ChunkAdoption,
        TickStage::Packets,
        TickStage::Save,
        TickStage::WorldClock,
        TickStage::Blocks,
        TickStage::Entities,
        TickStage::BlockEntities,
        TickStage::WorldAfter,
        TickStage::BlockChangeFlush,
        TickStage::LanGuests,
        TickStage::Paused,
        TickStage::Tracking,
        TickStage::ChunkSend,
        TickStage::LoadingScreen,
    ];

    fn name(self) -> &'static str {
        match self {
            TickStage::ChunkAdoption => "chunk_adoption",
            TickStage::Packets => "packets",
            TickStage::Save => "save",
            TickStage::WorldClock => "world_clock",
            TickStage::Blocks => "blocks",
            TickStage::Entities => "entities",
            TickStage::BlockEntities => "block_entities",
            TickStage::WorldAfter => "world_after",
            TickStage::BlockChangeFlush => "block_change_flush",
            TickStage::LanGuests => "lan_guests",
            TickStage::Paused => "paused",
            TickStage::Tracking => "tracking",
            TickStage::ChunkSend => "chunk_send",
            TickStage::LoadingScreen => "loading_screen",
        }
    }
}

/// Stage log on: `RUSTCRAFT_PERFLOG_STAGES=1`, read once.
fn enabled() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_PERFLOG_STAGES", default_off)
}

/// What one tick was made of (ns).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TickComposition {
    /// Server tick count at the start of the tick.
    pub tick: u64,
    pub total_ns: u64,
    pub stage_ns: [u64; STAGE_COUNT],
    /// Light computed on the server thread during the tick (inside the stages).
    pub light_ns: u64,
}

impl TickComposition {
    fn stage(&self, stage: TickStage) -> u64 {
        self.stage_ns[stage as usize]
    }

    /// Time of the tick outside every stage (between the last lap and the tick's end).
    fn rest_ns(&self) -> u64 {
        self.total_ns.saturating_sub(self.stage_ns.iter().sum())
    }
}

/// Ticks of a window, then the ticks that follow its slowest one.
const WINDOW_TICKS: usize = 100;
const FOLLOWERS: usize = 3;

/// Lap clock of the current tick and the ticks of the current window.
#[derive(Default)]
pub(crate) struct TickStageLog {
    /// Last stage boundary, `None` while the log is off (no clock read at all then).
    lap_at: Option<Instant>,
    stage_ns: [u64; STAGE_COUNT],
    light_ns_at_start: u64,
    window: SlowestTickWindow,
}

impl TickStageLog {
    /// Start of a tick at `at` (the instant its duration is measured from), when the log is on.
    pub(crate) fn start(&mut self, at: Instant) {
        if enabled() {
            self.start_measuring(at);
        }
    }

    fn start_measuring(&mut self, at: Instant) {
        self.lap_at = Some(at);
        self.stage_ns = [0; STAGE_COUNT];
        self.light_ns_at_start = rc_light::stats::this_thread_light_ns();
    }

    /// Charges the time since the previous boundary (or the start) to `stage`.
    pub(crate) fn lap(&mut self, stage: TickStage) {
        if let Some(last) = &mut self.lap_at {
            let now = Instant::now();
            self.stage_ns[stage as usize] += now.duration_since(*last).as_nanos() as u64;
            *last = now;
        }
    }

    /// End of the tick numbered `tick`, which lasted `total` from its start: its composition.
    fn finish(&mut self, tick: u64, total: Duration) -> Option<TickComposition> {
        self.lap_at.take()?;
        Some(TickComposition {
            tick,
            total_ns: total.as_nanos() as u64,
            stage_ns: self.stage_ns,
            light_ns: rc_light::stats::this_thread_light_ns() - self.light_ns_at_start,
        })
    }

    /// End of a tick: kept in the window, and the window's report written once it is complete.
    pub(crate) fn end_tick(&mut self, tick: u64, total: Duration) {
        let Some(composition) = self.finish(tick, total) else { return };
        if let Some(report) = self.window.push(composition) {
            let t = crate::generation_timing::seconds_since_origin(Instant::now());
            for line in report.lines(t) {
                eprintln!("{line}");
            }
        }
    }
}

/// Ticks of the current window, plus the ones after it that may follow its slowest tick.
#[derive(Default)]
struct SlowestTickWindow {
    ticks: Vec<TickComposition>,
}

/// The slowest tick of a window and the ticks right after it.
#[derive(Debug, PartialEq)]
struct SlowestTickReport {
    slowest: TickComposition,
    followers: Vec<TickComposition>,
}

impl SlowestTickWindow {
    /// Adds a tick; `Some` once the window and the followers of any of its ticks are in. The
    /// next window starts right after this one (the followers are its first ticks).
    fn push(&mut self, tick: TickComposition) -> Option<SlowestTickReport> {
        self.ticks.push(tick);
        if self.ticks.len() < WINDOW_TICKS + FOLLOWERS {
            return None;
        }
        // First of the equal maxima: the earliest spike of the window.
        let slowest = (0..WINDOW_TICKS).rev().max_by_key(|&i| self.ticks[i].total_ns)?;
        let report = SlowestTickReport {
            slowest: self.ticks[slowest],
            followers: self.ticks[slowest + 1..=slowest + FOLLOWERS].to_vec(),
        };
        self.ticks.drain(..WINDOW_TICKS);
        Some(report)
    }
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

impl SlowestTickReport {
    /// Counter lines then the readable line, dated `t` (s).
    fn lines(&self, t: f64) -> Vec<String> {
        let s = &self.slowest;
        let counter = |name: &str, value: f64| format!("[perf] [{t:>6.1} s] counter server_slowest_tick_{name} {value:.3}");
        let mut lines = vec![counter("ms", ms(s.total_ns))];
        lines.extend(TickStage::ALL.iter().map(|&stage| counter(&format!("{}_ms", stage.name()), ms(s.stage(stage)))));
        lines.push(counter("rest_ms", ms(s.rest_ns())));
        lines.push(counter("light_ms", ms(s.light_ns)));
        lines.push(counter("mod_400", (s.tick % 400) as f64));
        lines.push(counter("mod_20", (s.tick % 20) as f64));
        let after = self.followers.iter().map(|f| f.total_ns).sum::<u64>() / self.followers.len().max(1) as u64;
        lines.push(counter("followers_ms", ms(after)));
        let next: Vec<String> = self.followers.iter().map(composition_text).collect();
        lines.push(format!(
            "[perf] [{t:>6.1} s] server slowest tick {} (mod 400 = {}, mod 20 = {}): {}; next ticks: {}",
            s.tick,
            s.tick % 400,
            s.tick % 20,
            composition_text(s),
            next.join("; ")
        ));
        lines
    }
}

/// `12.345 ms = blocks 10.000 + entities 2.000 + rest 0.345, light 9.000`: the stages that took
/// time, slowest first.
fn composition_text(c: &TickComposition) -> String {
    let mut stages: Vec<(TickStage, u64)> =
        TickStage::ALL.iter().map(|&s| (s, c.stage(s))).filter(|&(_, ns)| ns >= 1_000).collect();
    stages.sort_by_key(|&(_, ns)| std::cmp::Reverse(ns));
    let mut parts: Vec<String> = stages.iter().map(|&(s, ns)| format!("{} {:.3}", s.name(), ms(ns))).collect();
    parts.push(format!("rest {:.3}", ms(c.rest_ns())));
    format!("{:.3} ms = {}, light {:.3}", ms(c.total_ns), parts.join(" + "), ms(c.light_ns))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(n: u64, stages: &[(TickStage, u64)]) -> TickComposition {
        let mut stage_ns = [0; STAGE_COUNT];
        for &(s, ns) in stages {
            stage_ns[s as usize] += ns;
        }
        TickComposition { tick: n, total_ns: stage_ns.iter().sum::<u64>() + 10, stage_ns, light_ns: 0 }
    }

    /// The report is one real tick, the slowest, with the three after it; a tick whose single stage
    /// is the window's biggest of its kind does not leak into it (no sum of the per-stage maxima).
    #[test]
    fn slowest_tick_is_reported_whole_with_its_followers() {
        let mut window = SlowestTickWindow::default();
        let mut report = None;
        for n in 0..(WINDOW_TICKS + FOLLOWERS) as u64 {
            let t = match n {
                40 => tick(n, &[(TickStage::Blocks, 9_000_000), (TickStage::Entities, 1_000_000)]),
                // Most entity time of the window, in a tick that is not the slowest.
                41 => tick(n, &[(TickStage::Entities, 5_000_000)]),
                _ => tick(n, &[(TickStage::Blocks, 100_000), (TickStage::Entities, 200_000)]),
            };
            report = window.push(t);
            if n < (WINDOW_TICKS + FOLLOWERS - 1) as u64 {
                assert!(report.is_none(), "report before the window was complete (tick {n})");
            }
        }
        let report = report.expect("report at the end of the window");
        assert_eq!(report.slowest.tick, 40);
        assert_eq!(report.slowest.stage(TickStage::Entities), 1_000_000, "the slowest tick's own entities");
        assert_eq!(report.followers.iter().map(|f| f.tick).collect::<Vec<_>>(), [41, 42, 43]);
        // The followers stay in for the next window, which starts right after this one.
        assert_eq!(window.ticks.len(), FOLLOWERS);
        assert_eq!(window.ticks[0].tick, WINDOW_TICKS as u64);
    }

    /// A slowest tick at the end of the window still gets its three followers.
    #[test]
    fn slowest_tick_at_the_window_end_keeps_its_followers() {
        let mut window = SlowestTickWindow::default();
        let mut last = None;
        for n in 0..(WINDOW_TICKS + FOLLOWERS) as u64 {
            let ns = if n == WINDOW_TICKS as u64 - 1 { 7_000_000 } else { 1_000 };
            last = window.push(tick(n, &[(TickStage::ChunkSend, ns)]));
        }
        let report = last.expect("report");
        assert_eq!(report.slowest.tick, WINDOW_TICKS as u64 - 1);
        assert_eq!(report.followers.len(), FOLLOWERS);
        // A spike among the followers belongs to the next window, not to this one.
        let mut window = SlowestTickWindow::default();
        let mut next_window_spike = None;
        for n in 0..(WINDOW_TICKS + FOLLOWERS) as u64 {
            let ns = if n == WINDOW_TICKS as u64 + 1 { 9_000_000 } else { 1_000 };
            next_window_spike = window.push(tick(n, &[(TickStage::ChunkSend, ns)]));
        }
        assert_ne!(next_window_spike.expect("report").slowest.tick, WINDOW_TICKS as u64 + 1);
    }

    /// Counter names read by the bench, each stage's own, the tick's phase, the readable line.
    #[test]
    fn report_lines_carry_every_stage_and_the_tick_phase() {
        let slowest = TickComposition { light_ns: 4_000_000, ..tick(1234, &[(TickStage::BlockChangeFlush, 6_000_000)]) };
        let report = SlowestTickReport { slowest, followers: vec![tick(1235, &[(TickStage::Blocks, 1_000_000)])] };
        let lines = report.lines(12.0);
        let has = |text: &str| lines.iter().any(|l| l == text);
        assert!(has("[perf] [  12.0 s] counter server_slowest_tick_ms 6.000"), "{lines:#?}");
        assert!(has("[perf] [  12.0 s] counter server_slowest_tick_block_change_flush_ms 6.000"));
        assert!(has("[perf] [  12.0 s] counter server_slowest_tick_entities_ms 0.000"));
        assert!(has("[perf] [  12.0 s] counter server_slowest_tick_light_ms 4.000"));
        assert!(has("[perf] [  12.0 s] counter server_slowest_tick_mod_400 34.000"));
        assert!(has("[perf] [  12.0 s] counter server_slowest_tick_mod_20 14.000"));
        assert_eq!(lines.iter().filter(|l| l.contains("counter server_slowest_tick_")).count(), STAGE_COUNT + 6);
        assert_eq!(
            lines.last().unwrap(),
            "[perf] [  12.0 s] server slowest tick 1234 (mod 400 = 34, mod 20 = 14): 6.000 ms = \
             block_change_flush 6.000 + rest 0.000, light 4.000; next ticks: 1.000 ms = blocks 1.000 + rest 0.000, light 0.000"
        );
    }

    /// Real ticks of a world with entities and flowing water: the stages cover the measured tick
    /// within 5 % and the work lands in its own stage. Fails if a lap forgets to move its boundary
    /// (time counted twice), or if a boundary is misplaced (entities charged elsewhere).
    #[test]
    fn stages_cover_real_ticks() {
        let (mut p, dir) = crate::tests_g3f13::open("tick-stages");
        for i in 0..40 {
            crate::bench_kit::cmd(&mut p, &format!("summon minecraft:zombie {} -60 {}", 2 + i % 8, 2 + i / 8));
        }
        crate::bench_kit::cmd(&mut p, "fill 20 -60 -4 24 -58 4 minecraft:water");
        let (mut covered, mut entity_ns, mut tick_ns) = (0, 0, 0);
        for _ in 0..40 {
            let start = Instant::now();
            p.s.stage_log.start_measuring(start);
            p.s.game_tick();
            let total = start.elapsed();
            p.s.tick += 1;
            let _ = p.take_events();
            let c = p.s.stage_log.finish(p.s.tick - 1, total).expect("tick measured");
            let sum: u64 = c.stage_ns.iter().sum();
            assert!(sum <= c.total_ns, "stages {sum} ns over the tick {} ns", c.total_ns);
            assert!(sum as f64 >= 0.95 * c.total_ns as f64, "stages {sum} ns cover under 95 % of {} ns", c.total_ns);
            assert!(c.light_ns <= c.total_ns, "light {} ns outside a tick of {} ns", c.light_ns, c.total_ns);
            for outside in [TickStage::Packets, TickStage::ChunkSend, TickStage::Save, TickStage::Paused] {
                assert_eq!(c.stage(outside), 0, "{} charged inside the world tick", outside.name());
            }
            covered += 1;
            entity_ns += c.stage(TickStage::Entities);
            tick_ns += c.total_ns;
        }
        assert_eq!(covered, 40);
        eprintln!("40 ticks: {:.3} ms in all, entities {:.3} ms", ms(tick_ns), ms(entity_ns));
        // 40 zombies are about half of these ticks (13.3 of 26.5 ms when written).
        assert!(entity_ns * 10 >= tick_ns, "entities {} ns of {} ns: their time went to another stage", entity_ns, tick_ns);
        crate::tests_g3f13::finish(p, dir);
    }

    /// Cost of the log for one tick (start, the 16 laps of the loop, end), on and off:
    /// `cargo test --release -p rc-sim tick_stage_log_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn tick_stage_log_cost() {
        const TICKS: u32 = 200_000;
        let run = |on: bool| {
            let mut log = TickStageLog::default();
            let started = Instant::now();
            for n in 0..TICKS {
                let at = Instant::now();
                if on {
                    log.start_measuring(at);
                }
                for &stage in TickStage::ALL.iter().chain(&[TickStage::ChunkAdoption, TickStage::Save]) {
                    log.lap(std::hint::black_box(stage));
                }
                if let Some(c) = log.finish(u64::from(n), at.elapsed())
                    && let Some(r) = log.window.push(c)
                {
                    std::hint::black_box(r);
                }
            }
            started.elapsed().as_nanos() as f64 / f64::from(TICKS)
        };
        let (off, on) = (run(false), run(true));
        eprintln!("[bench] tick_stage_log per_tick_ns off {off:.1}");
        eprintln!("[bench] tick_stage_log per_tick_ns on {on:.1}");
    }
}
