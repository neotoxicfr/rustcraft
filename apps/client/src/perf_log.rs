//! Measurement readings (`RUSTCRAFT_PERFLOG=<file>`, `docs/perf.md`): duration of
//! each frame (interval between two frames, presentation included) written to the file,
//! and every second on the error output a `[perf]` line: frame distribution
//! (median, 99th percentile, maximum), columns meshed, lighting (`rc_light::stats`, client and
//! integrated server), process memory, last instant when meshing was working, and once
//! the loading landmarks (world entered, first chunk received, in game, meshing caught up,
//! initial load meshed); then generic `counter <name> <value>` lines (column
//! `c:<name>` of `tools/bench/scenes.py`): GPU writes and draw-list stations per frame, visibility
//! verdicts, relights of the client and of the integrated server, light changes by origin,
//! propagation queues, mesh jobs by kind, base misses by cause, mesh cache shares and stages, ages
//! of the bases served, in-place uploads, column copies by writer, and the generic counters any
//! module adds by name ([`counter`], [`counter_per_second`], [`stage_time`], [`StageTimer`]) without
//! touching this file. Frames outside a world (title screen, menus, loading screen) are logged
//! too, without the world landmarks. The 99.9th percentile of the frames is a counter
//! (`frame_p999_ms`); with `RUSTCRAFT_PERFLOG_STAGES=1`, the composition of the frames and of the
//! slow ones (`frame_stages.rs`). Without the variable: one boolean read per frame.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::app_meshing::ChangeOrigin;
use crate::frame_stages::{self, FrameComposition, FrameStage, SecondComposition};

/// Columns meshed received by the render thread (up to date or stale).
static MESHED: AtomicU64 = AtomicU64::new(0);
/// Work of the pool threads on the two stations of remeshing: sections meshed and time spent
/// meshing, visibility walks redone and their time.
static SECTIONS: AtomicU64 = AtomicU64::new(0);
static MESH_NS: AtomicU64 = AtomicU64::new(0);
static GRAPHS: AtomicU64 = AtomicU64::new(0);
static GRAPH_NS: AtomicU64 = AtomicU64::new(0);

/// A mesh done (duration, sections actually meshed, kind of job).
pub fn mesh_work(d: std::time::Duration, sections: usize, kind: MeshJobKind) {
    let ns = d.as_nanos() as u64;
    SECTIONS.fetch_add(sections as u64, Relaxed);
    MESH_NS.fetch_add(ns, Relaxed);
    JOBS[kind as usize].fetch_add(1, Relaxed);
    JOB_NS[kind as usize].fetch_add(ns, Relaxed);
}

/// Bytes and columns of the mesh cache (`remesh.rs`).
static CACHE: AtomicU64 = AtomicU64::new(0);

pub fn mesh_cache(bytes: usize, columns: usize) {
    CACHE.store((bytes as u64) << 20 | columns as u64 & 0xf_ffff, Relaxed);
}

/// Partial remeshes launched with their base kept (success) or without (whole column).
static BASE_HIT: AtomicU64 = AtomicU64::new(0);
static BASE_MISS: AtomicU64 = AtomicU64::new(0);
/// Time of the pool threads to take back the memory returned by the main thread (`remesh::reclaim_pool`).
static RECLAIM_NS: AtomicU64 = AtomicU64::new(0);

pub fn reclaim_work(d: std::time::Duration) {
    RECLAIM_NS.fetch_add(d.as_nanos() as u64, Relaxed);
}

/// Time of the render thread to receive the meshes and send them to the GPU, split into reception
/// (`mesh_done`: base kept, checks) and upload (`upload`); columns sent, bytes and
/// in-place uploads; pool time to prepare the upload (`remesh::finish`).
static UPLOAD_NS: AtomicU64 = AtomicU64::new(0);
static DONE_NS: AtomicU64 = AtomicU64::new(0);
static SEND_NS: AtomicU64 = AtomicU64::new(0);
static SENT: AtomicU64 = AtomicU64::new(0);
static SENT_BYTES: AtomicU64 = AtomicU64::new(0);
static IN_PLACE: AtomicU64 = AtomicU64::new(0);
static PREP_NS: AtomicU64 = AtomicU64::new(0);

pub fn uploaded(total: std::time::Duration, done: std::time::Duration, send: std::time::Duration, columns: usize) {
    UPLOAD_NS.fetch_add(total.as_nanos() as u64, Relaxed);
    DONE_NS.fetch_add(done.as_nanos() as u64, Relaxed);
    SEND_NS.fetch_add(send.as_nanos() as u64, Relaxed);
    SENT.fetch_add(columns as u64, Relaxed);
}

/// Bytes of a column sent; `whole`: bytes of the whole column (an in-place write only sends
/// a part of them).
pub fn upload_bytes(bytes: usize, whole: usize, in_place: bool) {
    SENT_BYTES.fetch_add(bytes as u64, Relaxed);
    IN_PLACE.fetch_add(u64::from(in_place), Relaxed);
    if in_place {
        IN_PLACE_SENT_BYTES.fetch_add(bytes as u64, Relaxed);
        IN_PLACE_WHOLE_BYTES.fetch_add(whole as u64, Relaxed);
    }
}

/// GPU buffer writes of the renderer (`render/upload.rs`): calls, staging buffers created,
/// bytes; read per frame in the generic counters.
static GPU_WRITES: AtomicU64 = AtomicU64::new(0);
static GPU_TRANSITS: AtomicU64 = AtomicU64::new(0);
static GPU_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn gpu_writes(writes: u64, transits: u64, bytes: u64) {
    GPU_WRITES.fetch_add(writes, Relaxed);
    GPU_TRANSITS.fetch_add(transits, Relaxed);
    GPU_BYTES.fetch_add(bytes, Relaxed);
}

/// Translucent sections re-sorted (read per frame, to compare the re-sort rules).
static TRANS_RESORTS: AtomicU64 = AtomicU64::new(0);

pub fn trans_resorts(sections: usize) {
    TRANS_RESORTS.fetch_add(sections as u64, Relaxed);
}

/// Stations of the render thread's draw lists, every frame (cumulated ns, read as mean µs per
/// frame, frames without a rebuild included).
#[derive(Clone, Copy)]
pub enum ListStage {
    /// Radius of the walk (the render distance, read from the frame); printed `ray`, the name the
    /// reference logs use.
    Radius,
    /// Sections reached as a set, for the entity visibility.
    ReachedSet,
    /// Translucent re-sort (runs every frame, lists rebuilt or not).
    Resort,
    /// Lists rebuilt: view, order, batches.
    Lists,
    /// Indirect commands sent.
    CommandSubmit,
    /// Finished walks received, visibility snapshot, next walk launched.
    WalkLaunch,
}

const LIST_STAGES: [&str; 6] = ["ray", "reached", "resort", "lists", "command_submit", "walk_launch"];
static LIST_NS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

pub fn list_stage(stage: ListStage, d: std::time::Duration) {
    LIST_NS[stage as usize].fetch_add(d.as_nanos() as u64, Relaxed);
}

/// Time recorded so far for `stage` (ns), for the tests.
#[cfg(test)]
pub fn list_stage_ns(stage: ListStage) -> u64 {
    LIST_NS[stage as usize].load(Relaxed)
}

/// How a generic counter is printed: its sum over the window divided by the frames of the window,
/// or by its seconds; a time is summed in ns and printed in µs per frame.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Rate {
    PerFrame,
    PerSecond,
    MicrosPerFrame,
}

/// A named sum of the generic counters.
struct GenericCounter {
    name: &'static str,
    rate: Rate,
    sum: u64,
}

/// Generic counters in the order of their first use: printed every second as `counter <name>
/// <value>`, then reset to zero but kept (a counter that did not move prints 0).
static GENERIC: Mutex<Vec<GenericCounter>> = Mutex::new(Vec::new());

fn add_generic(name: &'static str, rate: Rate, value: u64) {
    let Ok(mut all) = GENERIC.lock() else { return };
    // A call site passes the same literal every time: a scan of the addresses alone finds it
    // without comparing any text; the text only settles a name written at two sites.
    let at = all.iter().position(|c| std::ptr::eq(c.name, name)).or_else(|| all.iter().position(|c| c.name == name));
    match at {
        Some(i) => all[i].sum += value,
        None => all.push(GenericCounter { name, rate, sum: value }),
    }
}

/// Generic counter: adds `value` to `name`, printed as its mean per frame (`counter <name>
/// <value>`, column `c:<name>` of `tools/bench/scenes.py`). Lets a mission add a reading without
/// touching this file; while the perf log is off, one cached boolean read. The rate of a name is
/// that of its first call.
pub fn counter(name: &'static str, value: u64) {
    if on() {
        add_generic(name, Rate::PerFrame, value);
    }
}

/// Generic counter printed per second (rare events, such as buffer growths).
pub fn counter_per_second(name: &'static str, value: u64) {
    if on() {
        add_generic(name, Rate::PerSecond, value);
    }
}

/// Generic time: adds `d` to `name`, printed in µs per frame (frames where it did not run included).
pub fn stage_time(name: &'static str, d: std::time::Duration) {
    if on() {
        add_generic(name, Rate::MicrosPerFrame, d.as_nanos() as u64);
    }
}

/// Splits a span of work into consecutive stages timed with [`stage_time`]: each `lap` charges
/// the time since the previous one to its stage. No clock read while the perf log is off.
pub struct StageTimer(Option<Instant>);

impl StageTimer {
    pub fn start() -> Self {
        StageTimer(on().then(Instant::now))
    }

    /// Charges the time since the previous lap (or the start) to `name`.
    pub fn lap(&mut self, name: &'static str) {
        if let Some(last) = &mut self.0 {
            let now = Instant::now();
            add_generic(name, Rate::MicrosPerFrame, now.duration_since(*last).as_nanos() as u64);
            *last = now;
        }
    }
}

/// Values of the generic counters over a window of `frames` frames and `window` seconds, sums
/// reset to zero.
fn drain_generic(frames: usize, window: f64) -> Vec<(&'static str, f64)> {
    let Ok(mut all) = GENERIC.lock() else { return Vec::new() };
    let frames = frames.max(1) as f64;
    all.iter_mut()
        .map(|c| {
            let sum = std::mem::take(&mut c.sum) as f64;
            let value = match c.rate {
                Rate::PerFrame => sum / frames,
                Rate::PerSecond => sum / window,
                Rate::MicrosPerFrame => sum / 1e3 / frames,
            };
            (c.name, value)
        })
        .collect()
}

pub fn prep_work(d: std::time::Duration) {
    PREP_NS.fetch_add(d.as_nanos() as u64, Relaxed);
}

/// A partial remesh launched with its base (`hit`) or without; `evicted`: the base was
/// dropped by the cache budget.
pub fn mesh_base(hit: bool, evicted: bool) {
    if hit { &BASE_HIT } else { &BASE_MISS }.fetch_add(1, Relaxed);
    if !hit {
        if evicted { &BASE_MISS_EVICTED } else { &BASE_MISS_NEVER_KEPT }.fetch_add(1, Relaxed);
    }
}

/// Reception of events by the render thread (server packets, chunks included): time
/// spent and number of events.
static RECV_NS: AtomicU64 = AtomicU64::new(0);
static RECV_EVENTS: AtomicU64 = AtomicU64::new(0);

pub fn received(d: std::time::Duration, events: usize) {
    if events > 0 {
        RECV_NS.fetch_add(d.as_nanos() as u64, Relaxed);
        RECV_EVENTS.fetch_add(events as u64, Relaxed);
    }
}

/// A visibility walk redone.
pub fn graph_work(d: std::time::Duration) {
    GRAPHS.fetch_add(1, Relaxed);
    GRAPH_NS.fetch_add(d.as_nanos() as u64, Relaxed);
}

/// Perf log on: the counters whose bookkeeping is not free (a set, a search, a table
/// lookup per change) are only kept then; the others are a relaxed addition.
pub fn on() -> bool {
    rc_data::env_flag!("RUSTCRAFT_PERFLOG")
}

/// Relights made by the client itself (main thread, `light_engine`): count, changes, time.
/// The rest of `rc_light::stats` belongs to the integrated server.
static CLIENT_RELIGHTS: AtomicU64 = AtomicU64::new(0);
static CLIENT_RELIGHT_CHANGES: AtomicU64 = AtomicU64::new(0);
static CLIENT_RELIGHT_NS: AtomicU64 = AtomicU64::new(0);
/// Same time, read back every frame by the frame composition (`frame_stages.rs`).
static FRAME_RELIGHT_NS: AtomicU64 = AtomicU64::new(0);

pub fn client_relight(d: std::time::Duration, changes: usize) {
    let ns = d.as_nanos() as u64;
    CLIENT_RELIGHTS.fetch_add(1, Relaxed);
    CLIENT_RELIGHT_CHANGES.fetch_add(changes as u64, Relaxed);
    CLIENT_RELIGHT_NS.fetch_add(ns, Relaxed);
    FRAME_RELIGHT_NS.fetch_add(ns, Relaxed);
}

/// Block changes that touch the light, by origin (`ChangeOrigin`): queued for the local
/// relight, or skipped because the integrated server has just sent their light.
static LIGHT_CHANGES: [AtomicU64; ChangeOrigin::ALL.len()] = [const { AtomicU64::new(0) }; ChangeOrigin::ALL.len()];

pub fn light_changes(origin: ChangeOrigin, n: usize) {
    LIGHT_CHANGES[origin as usize].fetch_add(n as u64, Relaxed);
}

/// Mesh jobs by kind (whole column, partial with its base, partial without it): count and
/// pool time, to price a base miss.
#[derive(Clone, Copy)]
pub enum MeshJobKind {
    Whole,
    PartialWithBase,
    PartialWithoutBase,
}

const JOB_KINDS: [&str; 3] = ["whole_column", "partial_with_base", "partial_without_base"];
static JOBS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static JOB_NS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// Partial remeshes without base, by cause: dropped by the cache to stay within its budget, or
/// never kept (the last mesh was a whole column, or too big for the budget). The cause is only
/// told apart while the perf log is on (`remesh::Meshes`).
static BASE_MISS_EVICTED: AtomicU64 = AtomicU64::new(0);
static BASE_MISS_NEVER_KEPT: AtomicU64 = AtomicU64::new(0);
/// Mesh cache gauges: bytes of fluid faces (27 words each, 6 for a face), and the columns whose
/// last mesh is a base, kept or evicted (the cache size that would lose none).
static CACHE_FLUID_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_COLUMNS: AtomicU64 = AtomicU64::new(0);

pub fn mesh_cache_shares(fluid_bytes: usize, live_columns: usize) {
    CACHE_FLUID_BYTES.store(fluid_bytes as u64, Relaxed);
    LIVE_COLUMNS.store(live_columns as u64, Relaxed);
}

/// Compressed bytes of the cold stage, and hot budget in force (moved by the regulator).
static CACHE_COLD_BYTES: AtomicU64 = AtomicU64::new(0);
static CACHE_BUDGET: AtomicU64 = AtomicU64::new(0);

pub fn mesh_cache_stages(cold_bytes: usize, budget: usize) {
    CACHE_COLD_BYTES.store(cold_bytes as u64, Relaxed);
    CACHE_BUDGET.store(budget as u64, Relaxed);
}

/// Bases served, by age of the base (time since its mesh was received): upper bounds of the
/// buckets in seconds, the last one open; and the bases served by the cold stage.
const BASE_AGE_BUCKETS: [(&str, u64); 5] =
    [("under_1s", 1), ("1_to_10s", 10), ("10_to_60s", 60), ("60_to_300s", 300), ("over_300s", u64::MAX)];
static BASE_AGE_HITS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static BASE_COLD_HITS: AtomicU64 = AtomicU64::new(0);

fn base_age_bucket(age: std::time::Duration) -> usize {
    BASE_AGE_BUCKETS.iter().position(|&(_, upper)| age.as_secs() < upper).unwrap_or(BASE_AGE_BUCKETS.len() - 1)
}

/// Pool time spent compressing the bases evicted into the cold stage.
static COLD_COMPRESS_NS: AtomicU64 = AtomicU64::new(0);

pub fn cold_compress_work(d: std::time::Duration) {
    COLD_COMPRESS_NS.fetch_add(d.as_nanos() as u64, Relaxed);
}

pub fn mesh_base_served(age: std::time::Duration, cold: bool) {
    BASE_AGE_HITS[base_age_bucket(age)].fetch_add(1, Relaxed);
    BASE_COLD_HITS.fetch_add(u64::from(cold), Relaxed);
}

/// Partial remeshes launched on a column the last visibility walk reached, or did not.
static PARTIALS_REACHED: AtomicU64 = AtomicU64::new(0);
static PARTIALS_UNREACHED: AtomicU64 = AtomicU64::new(0);

pub fn partial_reach(reached: bool) {
    if reached { &PARTIALS_REACHED } else { &PARTIALS_UNREACHED }.fetch_add(1, Relaxed);
}

/// Visibility filter of the meshing (`RUSTCRAFT_OPT_MESHVISIBLEONLY`): dirty columns held out of
/// view at the last scheduling, and launched columns that kept some marks (sections out of view).
static HELD_COLUMNS: AtomicU64 = AtomicU64::new(0);
static PARTLY_LAUNCHED: AtomicU64 = AtomicU64::new(0);

pub fn mesh_held(columns: usize, partly_launched: usize) {
    HELD_COLUMNS.store(columns as u64, Relaxed);
    PARTLY_LAUNCHED.fetch_add(partly_launched as u64, Relaxed);
}

/// In-place uploads: bytes really sent, bytes a whole upload of the same columns would have sent;
/// in-place writes refused because the GPU no longer holds the base version, or for another
/// reason (column absent, sizes changed).
static IN_PLACE_SENT_BYTES: AtomicU64 = AtomicU64::new(0);
static IN_PLACE_WHOLE_BYTES: AtomicU64 = AtomicU64::new(0);
static IN_PLACE_REFUSED_VERSION: AtomicU64 = AtomicU64::new(0);
static IN_PLACE_REFUSED_OTHER: AtomicU64 = AtomicU64::new(0);

pub fn in_place_refused(version_changed: bool) {
    if version_changed { &IN_PLACE_REFUSED_VERSION } else { &IN_PLACE_REFUSED_OTHER }.fetch_add(1, Relaxed);
}

/// Calls of the frame's visibility test (`render::visibility`), by verdict
/// (`render::BoxVerdict::ALL` order), read per frame.
static VISIBILITY: [AtomicU64; crate::render::BoxVerdict::ALL.len()] =
    [const { AtomicU64::new(0) }; crate::render::BoxVerdict::ALL.len()];

pub fn visibility_calls(per_verdict: &[u32]) {
    for (c, &n) in VISIBILITY.iter().zip(per_verdict) {
        if n > 0 {
            c.fetch_add(u64::from(n), Relaxed);
        }
    }
}

const WORK: usize = 17;

fn work_now() -> [u64; WORK] {
    [
        SECTIONS.load(Relaxed),
        MESH_NS.load(Relaxed),
        GRAPHS.load(Relaxed),
        GRAPH_NS.load(Relaxed),
        crate::debug_sys::process_cpu_ns(),
        RECV_NS.load(Relaxed),
        RECV_EVENTS.load(Relaxed),
        BASE_HIT.load(Relaxed),
        BASE_MISS.load(Relaxed),
        RECLAIM_NS.load(Relaxed),
        UPLOAD_NS.load(Relaxed),
        DONE_NS.load(Relaxed),
        SEND_NS.load(Relaxed),
        SENT.load(Relaxed),
        SENT_BYTES.load(Relaxed),
        IN_PLACE.load(Relaxed),
        PREP_NS.load(Relaxed),
    ]
}

struct State {
    out: std::io::BufWriter<std::fs::File>,
    last: Option<Instant>,
    /// Durations of the frames of the current second (ms).
    frames: Vec<f32>,
    /// The same frames with their composition (`RUSTCRAFT_PERFLOG_STAGES` only).
    compositions: Vec<FrameComposition>,
    since: Instant,
    meshed: u64,
    /// Sections, meshing ns, walks, walk ns, process CPU ns at the last reading.
    work: [u64; WORK],
    light: rc_light::stats::Totals,
    /// Propagation queues and column copies at the last reading (totals of the process).
    queues: rc_light::stats::Queues,
    copies: rc_world::ColumnCopies,
    /// Last instant (s since startup) when a meshing was in progress or to do.
    busy_at: f64,
    in_game: bool,
    /// The previous frame was drawn in a world (false on the title screen and the menus).
    in_world: bool,
    load: InitialLoad,
    /// Last `[perf] ram` line (every 5 s).
    ram_at: Instant,
}

impl State {
    /// Counter baselines taken now: the per-second lines that follow count from here.
    fn restart_counters(&mut self, now: Instant) {
        self.last = None;
        self.frames.clear();
        self.compositions.clear();
        self.since = now;
        self.work = work_now();
        self.light = rc_light::stats::totals();
        self.queues = rc_light::stats::queues();
        self.copies = rc_world::column_copies();
        self.ram_at = now;
    }
}

fn state() -> Option<&'static Mutex<State>> {
    static S: OnceLock<Option<Mutex<State>>> = OnceLock::new();
    S.get_or_init(|| {
        let path = rc_data::env_os!("RUSTCRAFT_PERFLOG")?;
        let f = std::fs::File::create(path).ok()?;
        Some(Mutex::new(State {
            out: std::io::BufWriter::with_capacity(1 << 16, f),
            last: None,
            frames: Vec::with_capacity(8192),
            compositions: Vec::new(),
            since: Instant::now(),
            meshed: 0,
            work: work_now(),
            light: rc_light::stats::totals(),
            queues: rc_light::stats::queues(),
            copies: rc_world::column_copies(),
            busy_at: 0.0,
            in_game: false,
            in_world: false,
            load: InitialLoad::default(),
            ram_at: Instant::now(),
        }))
    })
    .as_ref()
}

/// A column meshed received.
pub fn meshed(x: i32, z: i32) {
    MESHED.fetch_add(1, Relaxed);
    if let Some(m) = state()
        && let Ok(mut s) = m.lock()
    {
        s.load.column_meshed(x, z);
    }
}

/// A chunk received from the server: the first one of the process is a loading landmark.
pub fn chunk_received(started: Instant) {
    static SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // A plain load first: the swap (a write shared by every core) happens once.
    if on() && !SEEN.load(Relaxed) && !SEEN.swap(true, Relaxed) {
        eprintln!("[perf] first chunk received at {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
    }
}

/// What a frame drawn in a world tells the perf log.
pub struct WorldFrame {
    /// Meshes in progress or columns to mesh.
    pub busy: bool,
    /// Player placed and no screen outside the world.
    pub in_game: bool,
    /// Columns to mesh (loaded, minus those waiting for a neighbour).
    pub columns: usize,
}

/// End of a drawn frame: `world` is `None` on the title screen and the menus (frame interval and
/// per-second lines only).
pub fn frame(started: Instant, world: Option<WorldFrame>) {
    let Some(m) = state() else { return };
    let Ok(mut s) = m.lock() else { return };
    let now = Instant::now();
    let t = now.duration_since(started).as_secs_f64();
    let entering_world = world.is_some() && !s.in_world;
    s.in_world = world.is_some();
    if entering_world {
        // Same baselines as when the log only started in a world: the work done while the
        // loading screen was up (world preparation, spawn light) stays out of the world's lines.
        s.restart_counters(now);
        eprintln!("[perf] world entered at {:.0} ms", t * 1e3);
    }
    // Read at the same `now` as the interval: the stages of the frame add up to it exactly.
    // Read on every frame (the clock restarts here) but kept for the frames in a world only: out
    // of a world the menu frame runs inside `scheduledExecutables` and would accuse the light.
    let stage_ns = crate::profiler::take_frame_stages(now).filter(|_| world.is_some());
    let relight_ns = FRAME_RELIGHT_NS.swap(0, Relaxed);
    if let Some(last) = s.last.replace(now) {
        let ms = now.duration_since(last).as_secs_f64() * 1e3;
        s.frames.push(ms as f32);
        if let Some(stage_ns) = stage_ns {
            s.compositions.push(FrameComposition { ms: ms as f32, stage_ns, relight_ns });
        }
        let _ = writeln!(s.out, "{:.3} {ms:.4}", t * 1e3);
    }
    let cols = world.as_ref().map_or(0, |w| w.columns);
    if let Some(w) = &world {
        world_landmarks(&mut s, t, w);
    }
    let window = now.duration_since(s.since).as_secs_f64();
    if window < 1.0 {
        return;
    }
    let mut v = std::mem::take(&mut s.frames);
    v.sort_unstable_by(f32::total_cmp);
    let pick = |q: f64| v.get(((v.len() as f64 * q) as usize).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0);
    let (p50, p99, max) = (pick(0.5), pick(0.99), v.last().copied().unwrap_or(0.0));
    let p999 = pick(0.999);
    let n = v.len();
    v.clear();
    s.frames = v;
    let meshed = MESHED.load(Relaxed);
    let l = rc_light::stats::totals();
    let p = s.light;
    let (private, ws) = crate::debug_sys::process_memory();
    let mo = |b: u64| b as f64 / 1_048_576.0;
    eprintln!(
        "[perf] [{t:>6.1} s] frames {n}, median {p50:.3} ms, p99 {p99:.3} ms, max {max:.3} ms; columns meshed {:.0}/s; \
         light: {} chunks in {:.1} ms, {} relights ({} changes) in {:.1} ms; private memory {:.0} MB, \
         working set {:.0} MB; last mesh at {:.1} s; {cols} columns",
        (meshed - s.meshed) as f64 / window,
        l.chunks - p.chunks,
        (l.chunk_ns - p.chunk_ns) as f64 / 1e6,
        l.relights - p.relights,
        l.changes - p.changes,
        (l.relight_ns - p.relight_ns) as f64 / 1e6,
        mo(private),
        mo(ws),
        s.busy_at,
    );
    // Rest of the line (after the fields read by `tools/bench/scenes.py`).
    let w = work_now();
    let d: [f64; WORK] = std::array::from_fn(|i| (w[i] - s.work[i]) as f64 / window);
    eprintln!(
        "[perf] [{t:>6.1} s] work: sections meshed {:.0}/s in {:.1} ms/s; walks {:.1}/s in {:.2} ms/s; process CPU {:.0} ms/s; \
         mesh cache {:.1} MB, {} columns; receive {:.1} ms/s, {:.0} events/s; bases {:.1}/s, without base {:.1}/s; reclaim {:.2} ms/s; mesh upload {:.1} ms/s \
         (received {:.1} ms/s, GPU {:.1} ms/s, {:.0} columns/s, {:.1} MB/s, in place {:.1}/s); pool preparation {:.1} ms/s",
        d[0],
        d[1] / 1e6,
        d[2],
        d[3] / 1e6,
        d[4] / 1e6,
        (CACHE.load(Relaxed) >> 20) as f64 / 1_048_576.0,
        CACHE.load(Relaxed) & 0xf_ffff,
        d[5] / 1e6,
        d[6],
        d[7],
        d[8],
        d[9] / 1e6,
        d[10] / 1e6,
        d[11] / 1e6,
        d[12] / 1e6,
        d[13],
        d[14] / 1_048_576.0,
        d[15],
        d[16] / 1e6,
    );
    s.work = w;
    let per_frame = |c: &AtomicU64| c.swap(0, Relaxed) as f64 / n.max(1) as f64;
    for (name, c) in [
        ("gpu_writes_per_frame", &GPU_WRITES),
        ("gpu_transits_per_frame", &GPU_TRANSITS),
        ("translucent_resorts_per_frame", &TRANS_RESORTS),
    ] {
        eprintln!("[perf] [{t:>6.1} s] counter {name} {:.2}", per_frame(c));
    }
    eprintln!("[perf] [{t:>6.1} s] counter gpu_kb_written_per_frame {:.2}", per_frame(&GPU_BYTES) / 1024.0);
    for (name, c) in LIST_STAGES.iter().zip(&LIST_NS) {
        eprintln!("[perf] [{t:>6.1} s] counter us_{name}_per_frame {:.2}", per_frame(c) / 1e3);
    }
    for (verdict, c) in crate::render::BoxVerdict::ALL.iter().zip(&VISIBILITY) {
        print_counter(t, &format!("visibility_{}_per_frame", verdict.name()), per_frame(c));
    }
    for (name, value) in drain_generic(n, window) {
        print_counter(t, name, value);
    }
    // The 99.9th percentile next to the p99 and the max of the line above (a 1-in-1000 frame).
    print_counter(t, "frame_p999_ms", f64::from(p999));
    if !s.compositions.is_empty() {
        stage_counters(t, window, &SecondComposition::of(&s.compositions, p50));
        s.compositions.clear();
    }
    let queues = rc_light::stats::queues();
    light_counters(t, window, &l.since(&p), &queues.since(&s.queues));
    s.queues = queues;
    let copies = rc_world::column_copies();
    mesh_counters(t, window, &copies, &s.copies);
    s.copies = copies;
    // Memory per subsystem, every 5 s: process, allocator (private − committed by
    // mimalloc = graphics driver and other allocations outside our heap), integrated server.
    if now.duration_since(s.ram_at).as_secs_f64() >= 5.0 {
        s.ram_at = now;
        let (commit, peak) = crate::debug_sys::allocator_commit();
        #[cfg(feature = "memstats")]
        let live = format!(", live heap {:.0} MB", mo(crate::memstats::LIVE.load(Relaxed) as u64));
        #[cfg(not(feature = "memstats"))]
        let live = "";
        eprintln!(
            "[perf] [{t:>6.1} s] ram: private {:.0} MB, allocator {:.0} MB committed (peak {:.0}), outside the allocator {:.0} MB{live}; {}",
            mo(private),
            mo(commit),
            mo(peak),
            mo(private.saturating_sub(commit)),
            rc_sim::mem_stats::report()
        );
    }
    let _ = s.out.flush();
    s.meshed = meshed;
    s.light = l;
    s.since = now;
}

/// Once-per-process landmarks of a frame in a world: entry into the game, end of the initial load.
fn world_landmarks(s: &mut State, t: f64, w: &WorldFrame) {
    if w.busy {
        s.busy_at = t;
    }
    if w.in_game && !s.in_game {
        s.in_game = true;
        eprintln!("[perf] in game at {:.0} ms ({} columns)", t * 1e3, w.columns);
    }
    if let Some(done) = s.load.frame(t, s.in_game, w.columns) {
        eprintln!("[perf] meshing caught up at {:.0} ms", done.caught_up_at * 1e3);
        eprintln!(
            "[perf] initial load meshed at {:.0} ms ({} columns; column count stable since {:.0} ms)",
            done.meshed_at * 1e3,
            done.columns,
            done.stable_since * 1e3
        );
    }
}

/// Initial load of a world, as the bench reads it: done at the first frame in the game where every
/// column to mesh has been meshed at least once and their number has been stable for half a
/// second (remeshes happen afterwards as blocks change: meshing is never at rest for long).
#[derive(Default)]
struct InitialLoad {
    /// Columns to mesh and the instant (s) of their last change.
    columns: (usize, f64),
    /// Columns meshed at least once (until the load is done).
    seen: hashbrown::HashSet<(i32, i32)>,
    /// Instant (s) the meshing last caught up with the columns to mesh, while it still holds.
    caught_up_since: Option<f64>,
    done: bool,
}

/// Instants (s since startup) of a finished initial load.
#[derive(Debug, PartialEq)]
struct LoadLandmarks {
    meshed_at: f64,
    /// Last time every column to mesh became meshed at least once: the meshing's own end,
    /// without the half second of stable column count the load waits for.
    caught_up_at: f64,
    stable_since: f64,
    columns: usize,
}

impl InitialLoad {
    fn column_meshed(&mut self, x: i32, z: i32) {
        if !self.done {
            self.seen.insert((x, z));
        }
    }

    /// Frame at `t` (s); `Some` once, at the end of the load.
    fn frame(&mut self, t: f64, in_game: bool, columns: usize) -> Option<LoadLandmarks> {
        if columns != self.columns.0 {
            self.columns = (columns, t);
        }
        if !in_game || self.done {
            return None;
        }
        let caught_up = columns > 0 && self.seen.len() >= columns;
        self.caught_up_since = if caught_up { self.caught_up_since.or(Some(t)) } else { None };
        let caught_up_at = self.caught_up_since?;
        if t - self.columns.1 < 0.5 {
            return None;
        }
        self.done = true;
        self.seen = hashbrown::HashSet::new();
        Some(LoadLandmarks { meshed_at: t, caught_up_at, stable_since: self.columns.1, columns })
    }
}

/// Readings of the per-second statistics line, written in a world and outside (title screen,
/// menus). Its text is parsed by `tools/bench/scenes.py` (`STAT`) and `tools/bench/ab.py`.
pub struct SecondStats {
    pub fps: f64,
    pub cpu_ms: f64,
    pub gpu_ms: Option<f64>,
    pub columns_loaded: usize,
    pub columns_on_gpu: usize,
    pub faces_on_gpu: u64,
    pub columns_drawn: usize,
    pub faces_drawn: u64,
    pub meshes_in_progress: usize,
    pub sort_ms: f64,
    pub walk_ms: f64,
    /// Terrain regions allocated and used, textures, other (bytes).
    pub vram: (u64, u64, u64, u64),
    pub translucent_columns: usize,
    pub translucent_unsorted: usize,
    pub entities_drawn: usize,
    pub block_entities: usize,
    pub block_entities_drawn: usize,
}

impl SecondStats {
    /// Reads the renderer (its CPU time sum restarts at zero) and the counts of the world.
    pub fn read(
        r: &mut crate::render::Renderer,
        fps: f64,
        columns_loaded: usize,
        meshes_in_progress: usize,
        block_entities: usize,
    ) -> SecondStats {
        let (columns_on_gpu, faces_on_gpu) = r.stats();
        let (translucent_columns, translucent_unsorted) = r.trans_stats();
        SecondStats {
            fps,
            cpu_ms: r.cpu_ms(),
            gpu_ms: r.gpu_ms(),
            columns_loaded,
            columns_on_gpu,
            faces_on_gpu,
            columns_drawn: r.drawn.0,
            faces_drawn: r.drawn.1,
            meshes_in_progress,
            sort_ms: r.vis_ms,
            walk_ms: r.graph_ms,
            vram: r.vram(),
            translucent_columns,
            translucent_unsorted,
            entities_drawn: r.entities.as_ref().map_or(0, |e| e.drawn),
            block_entities,
            block_entities_drawn: r.entities.as_ref().map_or(0, |e| e.blocks.drawn),
        }
    }

    pub fn line(&self) -> String {
        let mb = |b: u64| b as f64 / 1_048_576.0;
        let (regions, used, textures, other) = self.vram;
        let gpu = self.gpu_ms.map_or_else(|| "?".into(), |ms| format!("{ms:.3}"));
        format!(
            "{:.0} fps, CPU {:.3} ms/frame, GPU {gpu} ms/frame, {} columns loaded, {} on GPU ({:.2} M faces), {} drawn ({:.2} M faces), {} meshes in progress, sort {:.3} ms (walk {:.3} ms), VRAM {:.1} MB (regions {:.1} of which {:.1} used, textures {:.1}, other {:.1}), translucent {} columns of which {} unsorted, {} entities drawn, block entities {} of which {} drawn",
            self.fps,
            self.cpu_ms,
            self.columns_loaded,
            self.columns_on_gpu,
            self.faces_on_gpu as f64 / 1e6,
            self.columns_drawn,
            self.faces_drawn as f64 / 1e6,
            self.meshes_in_progress,
            self.sort_ms,
            self.walk_ms,
            mb(regions + textures + other),
            mb(regions),
            mb(used),
            mb(textures),
            mb(other),
            self.translucent_columns,
            self.translucent_unsorted,
            self.entities_drawn,
            self.block_entities,
            self.block_entities_drawn
        )
    }
}

/// Generic line `[perf] [t s] counter <name> <value>`, read by `tools/bench/scenes.py` (column
/// `c:<name>`, mean over the measured window).
fn print_counter(t: f64, name: &str, value: f64) {
    eprintln!("[perf] [{t:>6.1} s] counter {name} {value:.2}");
}

/// Frame composition of the window (`RUSTCRAFT_PERFLOG_STAGES`): mean µs of every stage per frame,
/// then the same over the slow frames (interval over twice the median), and the slowest frame's
/// own composition in a readable line.
fn stage_counters(t: f64, window: f64, c: &SecondComposition) {
    for (stage, us) in FrameStage::ALL.iter().zip(&c.frame_us) {
        print_counter(t, &format!("frame_us_{}", stage.name()), *us);
    }
    print_counter(t, "frame_us_client_relight", c.relight_us);
    print_counter(t, "slow_frames_per_s", c.slow_frames as f64 / window);
    for (stage, us) in FrameStage::ALL.iter().zip(&c.slow_frame_us) {
        print_counter(t, &format!("slow_frame_us_{}", stage.name()), *us);
    }
    print_counter(t, "slow_frame_us_client_relight", c.slow_relight_us);
    if let Some(slowest) = &c.slowest {
        eprintln!("[perf] [{t:>6.1} s] slowest frame {}", frame_stages::composition_text(slowest));
    }
}

/// Light counters of the window: relights of the client and of the integrated server, light
/// changes by origin, propagation queues (entries per level, seeded or pushed, per kind of light).
fn light_counters(t: f64, window: f64, light: &rc_light::stats::Totals, queues: &rc_light::stats::Queues) {
    let rate = |c: &AtomicU64| c.swap(0, Relaxed) as f64 / window;
    let (relights, ns) = (rate(&CLIENT_RELIGHTS), rate(&CLIENT_RELIGHT_NS));
    print_counter(t, "client_relights_per_s", relights);
    print_counter(t, "client_relight_changes_per_s", rate(&CLIENT_RELIGHT_CHANGES));
    print_counter(t, "client_relight_ms_per_s", ns / 1e6);
    // The client's timer wraps the engine's: the difference may dip below zero by a few µs.
    print_counter(t, "server_relights_per_s", (light.relights as f64 / window - relights).max(0.0));
    print_counter(t, "server_relight_ms_per_s", ((light.relight_ns as f64 / window - ns) / 1e6).max(0.0));
    for origin in ChangeOrigin::ALL {
        print_counter(t, &format!("light_changes_{}_per_s", origin.name()), rate(&LIGHT_CHANGES[origin as usize]));
    }
    let per_s = |v: u64| (v as f64 / window).round() as u64;
    let mut levels = Vec::new();
    for (kind, k) in [("block", 0), ("sky", 1)] {
        let (seeded, queued): (u64, u64) = (queues.seeded[k].iter().sum(), queues.queued[k].iter().sum());
        print_counter(t, &format!("light_{kind}_seeded_per_s"), seeded as f64 / window);
        print_counter(t, &format!("light_{kind}_seeded_at_15_per_s"), queues.seeded[k][15] as f64 / window);
        print_counter(t, &format!("light_{kind}_pushed_per_s"), queued.saturating_sub(seeded) as f64 / window);
        print_counter(t, &format!("light_{kind}_cells_per_s"), queues.cells[k] as f64 / window);
        print_counter(t, &format!("light_{kind}_entries_per_cell"), queued as f64 / queues.cells[k].max(1) as f64);
        let by_level = |v: &[u64; 16]| v[1..].iter().rev().map(|&n| per_s(n).to_string()).collect::<Vec<_>>().join(" ");
        levels.push(format!("{kind} seeded {}, queued {}", by_level(&queues.seeded[k]), by_level(&queues.queued[k])));
    }
    eprintln!("[perf] [{t:>6.1} s] light queue entries per s, levels 15 to 1: {}", levels.join("; "));
}

/// Meshing counters of the window: jobs by kind, base misses by cause, mesh cache shares and stages,
/// ages of the bases served, cold compression time, partials outside the last walk, in-place
/// uploads, column copies by writer.
fn mesh_counters(t: f64, window: f64, copies: &rc_world::ColumnCopies, before: &rc_world::ColumnCopies) {
    let rate = |c: &AtomicU64| c.swap(0, Relaxed) as f64 / window;
    for (k, kind) in JOB_KINDS.iter().enumerate() {
        let (jobs, ns) = (JOBS[k].swap(0, Relaxed), JOB_NS[k].swap(0, Relaxed));
        print_counter(t, &format!("mesh_{kind}_jobs_per_s"), jobs as f64 / window);
        print_counter(t, &format!("mesh_{kind}_ms_per_job"), ns as f64 / 1e6 / jobs.max(1) as f64);
    }
    print_counter(t, "mesh_base_miss_evicted_per_s", rate(&BASE_MISS_EVICTED));
    print_counter(t, "mesh_base_miss_never_kept_per_s", rate(&BASE_MISS_NEVER_KEPT));
    print_counter(t, "mesh_cache_fluid_mb", CACHE_FLUID_BYTES.load(Relaxed) as f64 / 1_048_576.0);
    print_counter(t, "mesh_live_columns", LIVE_COLUMNS.load(Relaxed) as f64);
    print_counter(t, "mesh_cache_cold_mb", CACHE_COLD_BYTES.load(Relaxed) as f64 / 1_048_576.0);
    print_counter(t, "mesh_cache_budget_mb", CACHE_BUDGET.load(Relaxed) as f64 / 1_048_576.0);
    print_counter(t, "mesh_base_cold_hits_per_s", rate(&BASE_COLD_HITS));
    print_counter(t, "mesh_cold_compress_ms_per_s", rate(&COLD_COMPRESS_NS) / 1e6);
    for ((name, _), hits) in BASE_AGE_BUCKETS.iter().zip(&BASE_AGE_HITS) {
        print_counter(t, &format!("mesh_base_hits_age_{name}_per_s"), rate(hits));
    }
    print_counter(t, "mesh_partials_reached_per_s", rate(&PARTIALS_REACHED));
    print_counter(t, "mesh_partials_unreached_per_s", rate(&PARTIALS_UNREACHED));
    print_counter(t, "mesh_columns_held_out_of_view", HELD_COLUMNS.load(Relaxed) as f64);
    print_counter(t, "mesh_columns_partly_launched_per_s", rate(&PARTLY_LAUNCHED));
    print_counter(t, "upload_in_place_sent_mb_per_s", rate(&IN_PLACE_SENT_BYTES) / 1_048_576.0);
    print_counter(t, "upload_in_place_whole_mb_per_s", rate(&IN_PLACE_WHOLE_BYTES) / 1_048_576.0);
    print_counter(t, "upload_in_place_refused_version_per_s", rate(&IN_PLACE_REFUSED_VERSION));
    print_counter(t, "upload_in_place_refused_other_per_s", rate(&IN_PLACE_REFUSED_OTHER));
    for writer in rc_world::ColumnWriter::ALL {
        let k = writer as usize;
        print_counter(t, &format!("column_copies_{}_per_s", writer.name()), (copies[k] - before[k]) as f64 / window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(drained: &[(&'static str, f64)], name: &str) -> Option<f64> {
        drained.iter().find(|(n, _)| *n == name).map(|&(_, v)| v)
    }

    /// A drain empties every counter: the tests that drain take turns.
    static DRAINING: Mutex<()> = Mutex::new(());

    /// Each rate divides its sum by the right quantity; a drained counter is kept at zero.
    #[test]
    fn generic_counters_print_their_rate_then_restart_at_zero() {
        let _turn = DRAINING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        add_generic("test_items_per_frame", Rate::PerFrame, 6);
        add_generic("test_items_per_frame", Rate::PerFrame, 4);
        add_generic("test_events_per_s", Rate::PerSecond, 9);
        add_generic("test_us_per_frame", Rate::MicrosPerFrame, 3_000_000);
        let drained = drain_generic(4, 3.0);
        assert_eq!(value(&drained, "test_items_per_frame"), Some(2.5));
        assert_eq!(value(&drained, "test_events_per_s"), Some(3.0));
        assert_eq!(value(&drained, "test_us_per_frame"), Some(750.0));
        let again = drain_generic(4, 3.0);
        assert_eq!(value(&again, "test_items_per_frame"), Some(0.0), "a quiet counter still prints, at 0");
    }

    /// The same text from another literal (another address) reaches the same counter.
    #[test]
    fn generic_counters_are_found_by_name() {
        let _turn = DRAINING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let name: &'static str = String::from("test_same_name_per_frame").leak();
        add_generic("test_same_name_per_frame", Rate::PerFrame, 1);
        add_generic(name, Rate::PerFrame, 1);
        let drained = drain_generic(1, 1.0);
        assert_eq!(drained.iter().filter(|(n, _)| *n == name).count(), 1);
        assert_eq!(value(&drained, name), Some(2.0));
    }

    /// Cost of a generic counter while the perf log is on, with 40 counters registered (the name
    /// looked for last), and of a stage lap (two clock reads and an addition):
    /// `cargo test --release -p rc-app --bin rustcraft -- --ignored generic_counter_cost --nocapture`.
    #[test]
    #[ignore]
    fn generic_counter_cost() {
        let _turn = DRAINING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let names: Vec<&'static str> = (0..40).map(|i| &*format!("test_cost_{i}_per_frame").leak()).collect();
        for &n in &names {
            add_generic(n, Rate::PerFrame, 0);
        }
        const CALLS: u32 = 2_000_000;
        let started = Instant::now();
        for i in 0..CALLS {
            add_generic(names[39], Rate::PerFrame, u64::from(i & 1));
        }
        let add_ns = started.elapsed().as_nanos() as f64 / f64::from(CALLS);
        let mut timer = StageTimer(Some(Instant::now()));
        let started = Instant::now();
        for _ in 0..CALLS {
            timer.lap(names[39]);
        }
        let lap_ns = started.elapsed().as_nanos() as f64 / f64::from(CALLS);
        drain_generic(1, 1.0);
        eprintln!("generic counter: {add_ns:.1} ns per add, {lap_ns:.1} ns per stage lap (40 counters, last one)");
    }

    fn sample_stats(gpu_ms: Option<f64>) -> SecondStats {
        const MB: u64 = 1_048_576;
        SecondStats {
            fps: 143.6,
            cpu_ms: 0.41234,
            gpu_ms,
            columns_loaded: 625,
            columns_on_gpu: 600,
            faces_on_gpu: 1_234_567,
            columns_drawn: 300,
            faces_drawn: 456_789,
            meshes_in_progress: 3,
            sort_ms: 0.0123,
            walk_ms: 0.5,
            vram: (10 * MB, 5 * MB, 20 * MB, MB),
            translucent_columns: 12,
            translucent_unsorted: 4,
            entities_drawn: 7,
            block_entities: 9,
            block_entities_drawn: 2,
        }
    }

    /// The shared statistics line keeps, character for character, the text the in-world frame
    /// wrote before it was shared with the menus (`tools/bench/scenes.py` parses it).
    #[test]
    fn statistics_line_keeps_its_text() {
        assert_eq!(
            sample_stats(Some(0.1)).line(),
            "144 fps, CPU 0.412 ms/frame, GPU 0.100 ms/frame, 625 columns loaded, 600 on GPU (1.23 M faces), \
             300 drawn (0.46 M faces), 3 meshes in progress, sort 0.012 ms (walk 0.500 ms), VRAM 31.0 MB \
             (regions 10.0 of which 5.0 used, textures 20.0, other 1.0), translucent 12 columns of which 4 \
             unsorted, 7 entities drawn, block entities 9 of which 2 drawn"
        );
        assert!(sample_stats(None).line().starts_with("144 fps, CPU 0.412 ms/frame, GPU ? ms/frame, 625 columns"));
    }

    /// A frame of a load: `(t, in_game, columns, columns meshed during the frame)`.
    type LoadFrame<'a> = (f64, bool, usize, &'a [(i32, i32)]);

    fn play(frames: &[LoadFrame<'_>]) -> Vec<(f64, LoadLandmarks)> {
        let mut load = InitialLoad::default();
        let mut done = Vec::new();
        for &(t, in_game, columns, meshed) in frames {
            for &(x, z) in meshed {
                load.column_meshed(x, z);
            }
            done.extend(load.frame(t, in_game, columns).map(|l| (t, l)));
        }
        done
    }

    /// Done once, at the first frame that is caught up and half a second after the last change
    /// of the column count; the caught-up instant lies between them.
    #[test]
    fn initial_load_waits_for_stable_columns_after_catching_up() {
        let all: &[(i32, i32)] = &[(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)];
        let done = play(&[
            (0.1, false, 4, &all[..4]), // meshed before the game: counts, but not caught up yet
            (0.2, true, 4, &[]),        // caught up at the first frame in the game
            (1.0, true, 6, &[]),        // two more columns: no longer caught up
            (1.2, true, 6, &all[4..]),  // caught up again
            (1.4, true, 6, &[]),        // stable for 0.4 s only
            (1.5, true, 6, &[]),
            (2.5, true, 6, &[]), // already done: nothing more
        ]);
        let expected = LoadLandmarks { meshed_at: 1.5, caught_up_at: 1.2, stable_since: 1.0, columns: 6 };
        assert_eq!(done, vec![(1.5, expected)]);
    }

    /// The load ends at the later of the last catch-up and half a second of stable count
    /// (within a frame); the caught-up instant is the last one, not the first.
    #[test]
    fn initial_load_ends_at_the_later_of_catch_up_and_stable_count() {
        let done = play(&[
            (0.0, true, 1, &[(0, 0)]),
            (0.1, true, 2, &[]),
            (0.2, true, 2, &[(0, 1)]), // first catch-up
            (0.3, true, 3, &[]),
            (0.4, true, 3, &[(0, 2)]), // last catch-up
            (0.7, true, 3, &[]),
            (0.8, true, 3, &[]),
        ]);
        assert_eq!(done, vec![(0.8, LoadLandmarks { meshed_at: 0.8, caught_up_at: 0.4, stable_since: 0.3, columns: 3 })]);
        // Meshing slower than the half second: the load ends when it catches up.
        let done = play(&[(0.0, true, 2, &[(0, 0)]), (0.6, true, 2, &[]), (1.0, true, 2, &[(0, 1)])]);
        assert_eq!(done, vec![(1.0, LoadLandmarks { meshed_at: 1.0, caught_up_at: 1.0, stable_since: 0.0, columns: 2 })]);
        // A column count that drops while caught up: the catch-up stays the earlier instant.
        let done = play(&[(0.0, true, 2, &[(0, 0), (0, 1)]), (0.25, true, 1, &[]), (0.75, true, 1, &[])]);
        assert_eq!(done, vec![(0.75, LoadLandmarks { meshed_at: 0.75, caught_up_at: 0.0, stable_since: 0.25, columns: 1 })]);
        // Never done without a column, nor outside the game.
        assert!(play(&[(0.0, true, 0, &[]), (5.0, true, 0, &[])]).is_empty());
        assert!(play(&[(0.0, false, 1, &[(0, 0)]), (5.0, false, 1, &[])]).is_empty());
    }

    /// Ages of a base served sorted into their bucket, bounds excluded from the lower bucket.
    #[test]
    fn base_ages_sorted_into_buckets() {
        let bucket = |ms: u64| BASE_AGE_BUCKETS[base_age_bucket(std::time::Duration::from_millis(ms))].0;
        assert_eq!(bucket(0), "under_1s");
        assert_eq!(bucket(999), "under_1s");
        assert_eq!(bucket(1_000), "1_to_10s");
        assert_eq!(bucket(59_999), "10_to_60s");
        assert_eq!(bucket(299_000), "60_to_300s");
        assert_eq!(bucket(300_000), "over_300s");
        assert_eq!(bucket(u64::MAX), "over_300s");
    }
}
