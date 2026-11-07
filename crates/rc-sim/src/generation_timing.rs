//! Wall clock of each phase of a generation batch and of the generator setup, written under
//! `RUSTCRAFT_GENLOG` (measurement only: nothing here changes what is generated).
//!
//! A batch is cut into consecutive laps: each lap gives the time since the previous one to a
//! named phase, so the phases add up to the whole batch and nothing is left uncounted. The clock
//! reads the time once per phase boundary (about twenty reads per batch of 24 chunks), never per
//! chunk. Off (log absent), a lap is a single test of a boolean.
//!
//! The batch line is dated like the client's `[perf] [t s]` lines (seconds since the origin set
//! by [`set_log_origin`], the client's entry into `main`), so `tools/bench/scenes.py` can keep
//! only the batches of a scene's measurement window.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Origin of the `[t s]` dates of the generation lines.
static LOG_ORIGIN: OnceLock<Instant> = OnceLock::new();

/// Dates the generation lines from `origin` (the client calls it with the instant it dates its
/// own `[perf]` lines from). Without it, the first generation line sets the origin.
pub fn set_log_origin(origin: Instant) {
    let _ = LOG_ORIGIN.set(origin);
}

pub(crate) fn seconds_since_origin(now: Instant) -> f64 {
    now.duration_since(*LOG_ORIGIN.get_or_init(|| now)).as_secs_f64()
}

/// Generation log on (`RUSTCRAFT_GENLOG` present, read once).
pub(crate) fn generation_log() -> bool {
    rc_data::env_flag!("RUSTCRAFT_GENLOG")
}

/// Phases of a batch, in the order `Worker::process` runs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Chunks reread from memory or disk (neighbours of the targets and of those to decorate).
    Load,
    /// Biomes of the chunks whose terrain is missing.
    Biomes,
    /// Terrain and structure starts of the missing chunks (parallel).
    Terrain,
    /// Single-threaded glue between the phases (table inserts, lists, loading-screen map).
    Bookkeeping,
    /// Decoration, class by class (each class in parallel).
    Decoration,
    /// Post-processing of the targets, single-threaded (light of the chunks with mushrooms included).
    PostProcessing,
    /// Column scans, light and conversion to a full chunk of the targets (parallel).
    LightAndConversion,
    /// Light redone for the full neighbours of the newcomers (parallel).
    Relight,
    /// Full chunks and relights handed to the server.
    Sending,
    /// Proto-chunks encoded at every batch (old path `RUSTCRAFT_OPT_PROTO_LAZY=0`).
    ProtoChunks,
    /// Eviction of the table.
    Eviction,
}

pub(crate) const PHASES: [Phase; 11] = [
    Phase::Load,
    Phase::Biomes,
    Phase::Terrain,
    Phase::Bookkeeping,
    Phase::Decoration,
    Phase::PostProcessing,
    Phase::LightAndConversion,
    Phase::Relight,
    Phase::Sending,
    Phase::ProtoChunks,
    Phase::Eviction,
];

impl Phase {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Phase::Load => "load",
            Phase::Biomes => "biomes",
            Phase::Terrain => "terrain",
            Phase::Bookkeeping => "bookkeeping",
            Phase::Decoration => "decoration",
            Phase::PostProcessing => "post-processing",
            Phase::LightAndConversion => "light and conversion",
            Phase::Relight => "relight",
            Phase::Sending => "sending",
            Phase::ProtoChunks => "proto-chunks",
            Phase::Eviction => "eviction",
        }
    }

    /// Runs on the generation thread alone: the part of a batch that more threads cannot shorten
    /// (Amdahl). The other phases run parallel loops, with some single-threaded glue inside.
    pub(crate) fn single_threaded(self) -> bool {
        matches!(self, Phase::Bookkeeping | Phase::PostProcessing | Phase::Sending | Phase::Eviction)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Measured batch: wall time per phase, decoration per class, light CPU per phase.
#[derive(Clone, Debug, Default)]
pub(crate) struct BatchTiming {
    pub(crate) total: Duration,
    pub(crate) phases: [Duration; PHASES.len()],
    /// Decoration classes (or single chunks in sequential order): wall time and regions decorated.
    pub(crate) decoration_groups: Vec<(Duration, usize)>,
    /// Light work counted by `rc_light::stats` during each phase (CPU time summed over the threads;
    /// the counter is process-wide, so another generator running at the same time adds to it).
    pub(crate) light: [rc_light::stats::Totals; PHASES.len()],
    /// Targets whose post-processing needed their light (marked mushrooms).
    pub(crate) mushroom_chunks: usize,
}

impl BatchTiming {
    pub(crate) fn phase(&self, p: Phase) -> Duration {
        self.phases[p.index()]
    }

    /// Wall time of the single-threaded phases.
    pub(crate) fn single_threaded(&self) -> Duration {
        PHASES.iter().filter(|p| p.single_threaded()).map(|p| self.phase(*p)).sum()
    }

    /// One log line (parsed by `tools/bench/scenes.py`, keep the field order).
    pub(crate) fn line(&self, at_seconds: f64, batch: u64, chunks: usize, threads: usize) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let phases: Vec<String> = PHASES.iter().map(|p| format!("{} {:.2}", p.name(), ms(self.phase(*p)))).collect();
        let groups: Vec<String> = self.decoration_groups.iter().map(|(d, n)| format!("{:.2}/{n}", ms(*d))).collect();
        let light = |p: Phase| {
            let t = &self.light[p.index()];
            format!("{:.2} ({} chunks)", t.chunk_ns as f64 / 1e6, t.chunks)
        };
        format!(
            "[perf] [{at_seconds:>6.1} s] generation batch {batch}: {chunks} chunks, {:.2} ms, {threads} threads; \
             phases ms: {}; single-threaded {:.2} ms; decoration ms/regions: {}; light CPU ms: \
             post-processing {} with {} mushroom chunks, new {}, relit {}",
            ms(self.total),
            phases.join(", "),
            ms(self.single_threaded()),
            groups.join(" "),
            light(Phase::PostProcessing),
            self.mushroom_chunks,
            light(Phase::LightAndConversion),
            light(Phase::Relight),
        )
    }
}

/// Lap clock of a batch: `None` when the log is off (no clock read at all).
pub(crate) struct BatchClock {
    running: Option<Running>,
}

struct Running {
    started: Instant,
    last_lap: Instant,
    last_light: rc_light::stats::Totals,
    timing: BatchTiming,
}

impl BatchClock {
    pub(crate) fn start(on: bool) -> BatchClock {
        let running = on.then(|| {
            let now = Instant::now();
            Running {
                started: now,
                last_lap: now,
                last_light: rc_light::stats::totals(),
                timing: BatchTiming::default(),
            }
        });
        BatchClock { running }
    }

    /// Ends phase `p`: the time since the previous lap goes to it.
    pub(crate) fn lap(&mut self, p: Phase) {
        let Some(r) = &mut self.running else { return };
        let now = Instant::now();
        r.timing.phases[p.index()] += now - r.last_lap;
        r.last_lap = now;
        let light = rc_light::stats::totals();
        r.timing.light[p.index()] = r.timing.light[p.index()].plus(&light.since(&r.last_light));
        r.last_light = light;
    }

    /// Start of a decoration group (`None` when off).
    pub(crate) fn now(&self) -> Option<Instant> {
        self.running.as_ref().map(|_| Instant::now())
    }

    /// Decoration group started at `since` finished, `regions` regions decorated.
    pub(crate) fn decoration_group(&mut self, since: Option<Instant>, regions: usize) {
        if let (Some(r), Some(t)) = (&mut self.running, since) {
            r.timing.decoration_groups.push((t.elapsed(), regions));
        }
    }

    pub(crate) fn mushroom_chunk(&mut self) {
        if let Some(r) = &mut self.running {
            r.timing.mushroom_chunks += 1;
        }
    }

    /// Ends the batch at its last lap; `None` when off.
    pub(crate) fn finish(self) -> Option<(BatchTiming, f64)> {
        let mut r = self.running?;
        r.timing.total = r.last_lap - r.started;
        Some((r.timing, seconds_since_origin(r.last_lap)))
    }
}

/// Laps of the generator setup, always measured (once per dimension opened), printed under the log.
pub(crate) struct SetupClock {
    last_lap: Instant,
    laps: Vec<(&'static str, Duration)>,
}

impl SetupClock {
    pub(crate) fn start() -> SetupClock {
        SetupClock { last_lap: Instant::now(), laps: Vec::new() }
    }

    pub(crate) fn lap(&mut self, step: &'static str) {
        let now = Instant::now();
        self.laps.push((step, now - self.last_lap));
        self.last_lap = now;
    }

    pub(crate) fn line(&self, dimension: &str) -> String {
        let laps: Vec<String> = self.laps.iter().map(|(s, d)| format!("{s} {:.1}", d.as_secs_f64() * 1e3)).collect();
        format!("singleplayer: generator {dimension} setup ms: {}", laps.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn laps_cover_the_whole_batch() {
        let mut c = BatchClock::start(true);
        for p in PHASES {
            std::thread::sleep(Duration::from_micros(200));
            c.lap(p);
        }
        let g = c.now();
        c.decoration_group(g, 3);
        c.mushroom_chunk();
        let (t, _) = c.finish().expect("clock on");
        let sum: Duration = t.phases.iter().sum();
        assert_eq!(sum, t.total, "phases must add up to the batch");
        assert!(t.phases.iter().all(|d| *d >= Duration::from_micros(200)), "{:?}", t.phases);
        assert_eq!((t.decoration_groups.len(), t.decoration_groups[0].1, t.mushroom_chunks), (1, 3, 1));
        assert!(t.single_threaded() < t.total);
    }

    #[test]
    fn clock_off_records_nothing() {
        let mut c = BatchClock::start(false);
        c.lap(Phase::Terrain);
        assert!(c.now().is_none());
        c.decoration_group(Some(Instant::now()), 1);
        c.mushroom_chunk();
        assert!(c.finish().is_none());
    }

    /// Micro-bench (by hand, `--ignored --nocapture`): cost of the log on one batch (laps of every
    /// phase, nine decoration classes, the line built), to set against a batch of ~37 ms.
    #[test]
    #[ignore]
    fn bench_batch_clock_cost() {
        const BATCHES: u32 = 20_000;
        for on in [false, true] {
            let t = Instant::now();
            let mut length = 0;
            for _ in 0..BATCHES {
                let mut c = BatchClock::start(on);
                for p in PHASES {
                    c.lap(p);
                }
                for _ in 0..9 {
                    let g = c.now();
                    c.decoration_group(g, 4);
                }
                if let Some((timing, at)) = c.finish() {
                    length += timing.line(at, 1, 24, 32).len();
                }
            }
            let per_batch = t.elapsed().as_secs_f64() * 1e6 / f64::from(BATCHES);
            eprintln!("log {}: {per_batch:.2} µs per batch ({length} bytes of lines)", if on { "on" } else { "off" });
        }
    }

    /// The bench parser (`tools/bench/scenes.py`, `GEN_BATCH`) reads this exact shape.
    #[test]
    fn batch_line_shape() {
        let mut t = BatchTiming { total: Duration::from_micros(3_500), ..Default::default() };
        t.phases[Phase::Terrain.index()] = Duration::from_micros(2_000);
        t.phases[Phase::PostProcessing.index()] = Duration::from_micros(1_500);
        t.decoration_groups = vec![(Duration::from_micros(250), 4)];
        let l = t.line(12.34, 7, 24, 32);
        assert!(l.starts_with("[perf] [  12.3 s] generation batch 7: 24 chunks, 3.50 ms, 32 threads; phases ms: load 0.00, biomes 0.00, terrain 2.00, "), "{l}");
        assert!(l.contains("eviction 0.00; single-threaded 1.50 ms; decoration ms/regions: 0.25/4; light CPU ms: post-processing 0.00 (0 chunks) with 0 mushroom chunks"), "{l}");
    }
}
