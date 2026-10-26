//! Measurement readings (`RUSTCRAFT_PERFLOG=<file>`, `docs/perf.md`): duration of
//! each frame (interval between two frames, presentation included) written to the file,
//! and every second on the error output a `[perf]` line: frame distribution
//! (median, 99th percentile, maximum), columns meshed, lighting (`rc_light::stats`, client and
//! integrated server), process memory, last instant when meshing was working, and once
//! the instant of entering the game. Without the variable: one boolean read per frame.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Columns meshed received by the render thread (up to date or stale).
static MESHED: AtomicU64 = AtomicU64::new(0);
/// Work of the pool threads on the two stations of remeshing: sections meshed and time spent
/// meshing, visibility walks redone and their time.
static SECTIONS: AtomicU64 = AtomicU64::new(0);
static MESH_NS: AtomicU64 = AtomicU64::new(0);
static GRAPHS: AtomicU64 = AtomicU64::new(0);
static GRAPH_NS: AtomicU64 = AtomicU64::new(0);

/// A mesh done (duration, sections actually meshed).
pub fn mesh_work(d: std::time::Duration, sections: usize) {
    SECTIONS.fetch_add(sections as u64, Relaxed);
    MESH_NS.fetch_add(d.as_nanos() as u64, Relaxed);
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

/// Bytes of a column sent (prepared; an in-place write only sends a part of them).
pub fn upload_bytes(bytes: usize, in_place: bool) {
    SENT_BYTES.fetch_add(bytes as u64, Relaxed);
    IN_PLACE.fetch_add(u64::from(in_place), Relaxed);
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

/// Stations of the render thread's draw lists (cumulated ns, read as mean µs per frame,
/// frames without a rebuild included): ray, sections reached, translucent re-sort, lists
/// (view, order, batches), command submission.
const LIST_STAGES: [&str; 5] = ["ray", "reached", "resort", "lists", "command_submit"];
static LIST_NS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

pub fn list_stage(stage: usize, d: std::time::Duration) {
    LIST_NS[stage].fetch_add(d.as_nanos() as u64, Relaxed);
}

pub fn prep_work(d: std::time::Duration) {
    PREP_NS.fetch_add(d.as_nanos() as u64, Relaxed);
}

pub fn mesh_base(hit: bool) {
    if hit { &BASE_HIT } else { &BASE_MISS }.fetch_add(1, Relaxed);
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
    since: Instant,
    meshed: u64,
    /// Sections, meshing ns, walks, walk ns, process CPU ns at the last reading.
    work: [u64; WORK],
    light: rc_light::stats::Totals,
    /// Last instant (s since startup) when a meshing was in progress or to do.
    busy_at: f64,
    in_game: bool,
    /// Columns loaded and instant of their last change (s).
    cols: (usize, f64),
    settled: bool,
    /// Columns meshed at least once (until the initial load).
    seen: hashbrown::HashSet<(i32, i32)>,
    /// Last `[perf] ram` line (every 5 s).
    ram_at: Instant,
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
            since: Instant::now(),
            meshed: 0,
            work: work_now(),
            light: rc_light::stats::totals(),
            busy_at: 0.0,
            in_game: false,
            cols: (0, 0.0),
            settled: false,
            seen: hashbrown::HashSet::new(),
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
        && !s.settled
    {
        s.seen.insert((x, z));
    }
}

/// End of a drawn frame. `busy`: meshes in progress or columns to mesh; `in_game`:
/// player placed and no screen outside the world; `cols`: columns loaded.
pub fn frame(started: Instant, busy: bool, in_game: bool, cols: usize) {
    let Some(m) = state() else { return };
    let Ok(mut s) = m.lock() else { return };
    let now = Instant::now();
    let t = now.duration_since(started).as_secs_f64();
    if let Some(last) = s.last.replace(now) {
        let ms = now.duration_since(last).as_secs_f64() * 1e3;
        s.frames.push(ms as f32);
        let _ = writeln!(s.out, "{:.3} {ms:.4}", t * 1e3);
    }
    if busy {
        s.busy_at = t;
    }
    if cols != s.cols.0 {
        s.cols = (cols, t);
    }
    if in_game && !s.in_game {
        s.in_game = true;
        eprintln!("[perf] in game at {:.0} ms ({cols} columns)", t * 1e3);
    }
    // Initial load: first frame where every loaded column has been meshed at least once
    // and the number of columns has been stable for half a second (remeshes happen
    // afterwards as blocks change: meshing is never at rest for long).
    if s.in_game && !s.settled && cols > 0 && s.seen.len() >= cols && t - s.cols.1 >= 0.5 {
        s.settled = true;
        s.seen = hashbrown::HashSet::new();
        eprintln!(
            "[perf] initial load meshed at {:.0} ms ({cols} columns; column count stable since {:.0} ms)",
            t * 1e3,
            s.cols.1 * 1e3
        );
    }
    let window = now.duration_since(s.since).as_secs_f64();
    if window < 1.0 {
        return;
    }
    let mut v = std::mem::take(&mut s.frames);
    v.sort_unstable_by(f32::total_cmp);
    let pick = |q: f64| v.get(((v.len() as f64 * q) as usize).min(v.len().saturating_sub(1))).copied().unwrap_or(0.0);
    let (p50, p99, max) = (pick(0.5), pick(0.99), v.last().copied().unwrap_or(0.0));
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
