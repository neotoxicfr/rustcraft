//! GPU time of a frame, read back later (ring of buffers, never a blocking wait).
//!
//! Frame total (`RUSTCRAFT_OPT_GPUFRAME`, default 1): from the encoder's first command
//! to the last, all passes included (item atlas, world, transparency, effects, hand,
//! interface, blur). Two ways, depending on the adapter:
//! - timestamps in the encoder (`TIMESTAMP_QUERY_INSIDE_ENCODERS`), independent of the
//!   variable order of the passes;
//! - otherwise start of the first encoded pass and end of the last (no empty pass added).
//!
//! `RUSTCRAFT_OPT_GPUFRAME=0` goes back to the old measurement: the world pass alone.
//!
//! Diagnostic `RUSTCRAFT_GPUPASSES=1` (default 0): a pair of timestamps per named pass,
//! accumulated by name, and the fragment count of the world pass when the adapter offers
//! pipeline statistics; one "GPU passes" line per sample, apart from the main line.
//!
//! On Vulkan, resolving waits for every query of the range: only queries written in
//! the frame are resolved (`Plan::written`), otherwise the GPU would wait forever.

use std::cell::Cell;
use std::ops::Range;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

/// Timestamped named passes at most per frame (beyond: not measured, counted apart; the
/// last pair always goes to the last pass).
pub const MAX_PASSES: usize = 64;
/// Queries: the frame's pair (0, 1), then one pair per pass (2 + 2k, 3 + 2k).
const QUERIES: u32 = 2 + 2 * MAX_PASSES as u32;
/// Pipeline statistics in the buffers: after the timestamps, aligned to 256 bytes.
const STATS_OFFSET: u64 = (QUERIES as u64 * 8).div_ceil(256) * 256;
const STATS_BYTES: u64 = 16;
const RING: usize = 4;

const IDLE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;

/// Switches read once at startup.
pub struct Settings {
    /// Total of the whole frame (`RUSTCRAFT_OPT_GPUFRAME`, default 1); otherwise world pass alone.
    pub whole_frame: bool,
    /// Breakdown per pass (`RUSTCRAFT_GPUPASSES=1`).
    pub passes: bool,
}

pub fn settings() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    S.get_or_init(|| Settings {
        whole_frame: rc_data::env_str!("RUSTCRAFT_OPT_GPUFRAME") != Some("0"),
        passes: rc_data::env_str!("RUSTCRAFT_GPUPASSES") == Some("1"),
    })
}

/// Origin of the frame total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameMode {
    /// Timestamps in the encoder, before the first command and after the last.
    Encoder,
    /// Start of the first pass, end of the last.
    Passes,
    /// Old measurement: world pass alone.
    World,
}

impl FrameMode {
    pub fn describe(self) -> &'static str {
        match self {
            FrameMode::Encoder => "whole frame (encoder)",
            FrameMode::Passes => "whole frame (first to last pass)",
            FrameMode::World => "world pass only",
        }
    }
}

/// Place of a pass in the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Inner,
    /// World pass (total of the old measurement).
    World,
    /// Last pass encoded in the frame.
    Last,
}

/// What a frame wrote, kept with its readback buffer.
#[derive(Clone)]
pub struct Meta {
    n: u32,
    frame: (Option<u32>, Option<u32>),
    range: Range<u32>,
    names: [&'static str; MAX_PASSES],
    stats: bool,
    dropped: u32,
}

/// Query indices of the current frame (without GPU: tested alone).
pub struct Plan {
    mode: FrameMode,
    passes: bool,
    active: Cell<bool>,
    n: Cell<u32>,
    first_done: Cell<bool>,
    frame: Cell<(Option<u32>, Option<u32>)>,
    names: [Cell<&'static str>; MAX_PASSES],
    stats: Cell<bool>,
    dropped: Cell<u32>,
}

impl Plan {
    pub fn new(mode: FrameMode, passes: bool) -> Plan {
        Plan {
            mode,
            passes,
            active: Cell::new(false),
            n: Cell::new(0),
            first_done: Cell::new(false),
            frame: Cell::new((None, None)),
            names: std::array::from_fn(|_| Cell::new("")),
            stats: Cell::new(false),
            dropped: Cell::new(0),
        }
    }

    /// Start of a measured frame.
    pub fn start(&self) {
        self.active.set(true);
        self.n.set(0);
        self.first_done.set(false);
        self.stats.set(false);
        self.dropped.set(0);
        self.frame.set(if self.mode == FrameMode::Encoder { (Some(0), None) } else { (None, None) });
    }

    /// End of the frame: no more timestamped passes.
    pub fn finish(&self) -> Meta {
        if self.mode == FrameMode::Encoder {
            self.frame.set((Some(0), Some(1)));
        }
        self.active.set(false);
        Meta {
            n: self.n.get(),
            frame: self.frame.get(),
            range: self.written(),
            names: std::array::from_fn(|k| self.names[k].get()),
            stats: self.stats.get(),
            dropped: self.dropped.get(),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.get()
    }

    /// Start and end indices to write for a pass (`None`: pass not timestamped).
    pub fn assign(&self, name: &'static str, role: Role) -> Option<(Option<u32>, Option<u32>)> {
        if !self.active.get() {
            return None;
        }
        let (mut b, mut e) = (None, None);
        let mut fr = self.frame.get();
        let first = !self.first_done.get();
        self.first_done.set(true);
        if self.passes {
            let k = self.n.get();
            // Last pair kept for the last pass: the total stays defined if the sequence overflows.
            let cap = if role == Role::Last { MAX_PASSES } else { MAX_PASSES - 1 };
            if (k as usize) < cap {
                self.n.set(k + 1);
                self.names[k as usize].set(name);
                (b, e) = (Some(2 + 2 * k), Some(3 + 2 * k));
            } else {
                self.dropped.set(self.dropped.get() + 1);
            }
            match self.mode {
                FrameMode::World if role == Role::World => fr = (b, e),
                FrameMode::Passes => {
                    if first {
                        fr.0 = b;
                    }
                    if role == Role::Last {
                        fr.1 = e;
                    }
                }
                _ => {}
            }
        } else {
            match self.mode {
                FrameMode::World if role == Role::World => {
                    (b, e) = (Some(0), Some(1));
                    fr = (b, e);
                }
                FrameMode::Passes => {
                    if first {
                        b = Some(0);
                        fr.0 = b;
                    }
                    if role == Role::Last {
                        e = Some(1);
                        fr.1 = e;
                    }
                }
                _ => {}
            }
        }
        self.frame.set(fr);
        (b.is_some() || e.is_some()).then_some((b, e))
    }

    /// Pipeline statistics taken in this frame.
    pub fn mark_stats(&self) {
        self.stats.set(true);
    }

    /// Contiguous range of the queries written in the frame (the only one resolved).
    pub fn written(&self) -> Range<u32> {
        let pairs = 2 + 2 * self.n.get();
        if self.passes {
            if self.mode == FrameMode::Encoder { 0..pairs } else { 2..pairs }
        } else {
            match (self.mode, self.frame.get()) {
                (FrameMode::Encoder, _) => 0..2,
                (_, (Some(_), Some(_))) => 0..2,
                (_, (Some(_), None)) => 0..1,
                _ => 0..0,
            }
        }
    }
}

/// Measurements accumulated since the last sample.
pub struct Report {
    total_ms: f64,
    total_frames: u32,
    pass_frames: u32,
    names: [&'static str; MAX_PASSES],
    ms: [f64; MAX_PASSES],
    len: usize,
    dropped: u32,
    fragments: u64,
    primitives: u64,
    stat_frames: u32,
}

impl Default for Report {
    fn default() -> Self {
        Report {
            total_ms: 0.0,
            total_frames: 0,
            pass_frames: 0,
            names: [""; MAX_PASSES],
            ms: [0.0; MAX_PASSES],
            len: 0,
            dropped: 0,
            fragments: 0,
            primitives: 0,
            stat_frames: 0,
        }
    }
}

impl Report {
    /// Adds a frame: `ticks` = values of `meta.range`, `stats` = (primitives, fragments).
    pub fn add(&mut self, meta: &Meta, ticks: &[u64], period_ns: f64, stats: Option<[u64; 2]>) {
        let at = |i: u32| i.checked_sub(meta.range.start).and_then(|j| ticks.get(j as usize)).copied();
        let ms = |a: u64, b: u64| b.saturating_sub(a) as f64 * period_ns / 1e6;
        if let (Some(b), Some(e)) = meta.frame
            && let (Some(tb), Some(te)) = (at(b), at(e))
            && te > tb
        {
            self.total_ms += ms(tb, te);
            self.total_frames += 1;
        }
        if meta.n > 0 {
            self.pass_frames += 1;
            for k in 0..meta.n {
                let (Some(tb), Some(te)) = (at(2 + 2 * k), at(3 + 2 * k)) else { continue };
                let name = meta.names[k as usize];
                let slot = match self.names[..self.len].iter().position(|n| *n == name) {
                    Some(i) => i,
                    None if self.len < MAX_PASSES => {
                        self.names[self.len] = name;
                        self.len += 1;
                        self.len - 1
                    }
                    None => continue,
                };
                self.ms[slot] += ms(tb, te);
            }
        }
        self.dropped += meta.dropped;
        if meta.stats
            && let Some([p, f]) = stats
        {
            self.primitives += p;
            self.fragments += f;
            self.stat_frames += 1;
        }
    }

    /// Mean total per frame, then reset of the total.
    pub fn take_total(&mut self) -> Option<f64> {
        let avg = (self.total_frames > 0).then(|| self.total_ms / f64::from(self.total_frames));
        self.total_ms = 0.0;
        self.total_frames = 0;
        avg
    }

    /// Means per frame: total, passes (in order of first appearance).
    pub fn passes(&self) -> impl Iterator<Item = (&'static str, f64)> + '_ {
        let f = f64::from(self.pass_frames.max(1));
        self.names[..self.len].iter().zip(&self.ms).map(move |(n, ms)| (*n, ms / f))
    }

    /// Log line (ms per frame), `total` already taken; `pixels`: frame size.
    pub fn line(&self, total: Option<f64>, mode: FrameMode, pixels: u64) -> String {
        use std::fmt::Write;
        let mut s = format!("GPU passes ({} frames): total ", self.pass_frames);
        match total {
            Some(t) => {
                let _ = write!(s, "{t:.3}");
            }
            None => s.push('?'),
        }
        let mut sum = 0.0;
        for (name, ms) in self.passes() {
            sum += ms;
            let _ = write!(s, " ; {name} {ms:.3}");
        }
        // Outside passes (barriers, blur and effects copies): only if the total covers the frame.
        if let Some(t) = total.filter(|_| mode != FrameMode::World) {
            let _ = write!(s, "; outside passes {:.3}", t - sum);
        }
        if self.dropped > 0 {
            let _ = write!(s, "; unmeasured passes {}", self.dropped);
        }
        if self.stat_frames > 0 {
            let n = f64::from(self.stat_frames);
            let frags = self.fragments as f64 / n;
            let _ = write!(
                s,
                "; world: {:.3} M fragments, overdraw {:.2}, {:.3} M triangles",
                frags / 1e6,
                frags / pixels.max(1) as f64,
                self.primitives as f64 / n / 1e6
            );
        }
        s
    }

    pub fn reset_passes(&mut self) {
        *self = Report { total_ms: self.total_ms, total_frames: self.total_frames, ..Report::default() };
    }
}

struct Slot {
    buf: wgpu::Buffer,
    state: Arc<AtomicU8>,
    meta: Option<Meta>,
}

/// GPU timestamps of the frame and its passes.
pub struct GpuTimer {
    plan: Plan,
    queries: wgpu::QuerySet,
    stats: Option<wgpu::QuerySet>,
    resolve: wgpu::Buffer,
    ring: Vec<Slot>,
    next: usize,
    current: Option<usize>,
    period_ns: f64,
    report: Report,
}

impl GpuTimer {
    /// Mode chosen from the features opened and the switches.
    pub fn mode_for(features: wgpu::Features) -> FrameMode {
        if !settings().whole_frame {
            FrameMode::World
        } else if features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
            FrameMode::Encoder
        } else {
            FrameMode::Passes
        }
    }

    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, mode: FrameMode, passes: bool) -> Self {
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("GPU time"),
            ty: wgpu::QueryType::Timestamp,
            count: if passes { QUERIES } else { 2 },
        });
        let stats = (passes && device.features().contains(wgpu::Features::PIPELINE_STATISTICS_QUERY)).then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("world statistics"),
                ty: wgpu::QueryType::PipelineStatistics(
                    wgpu::PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT
                        | wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS,
                ),
                count: 1,
            })
        });
        let size = if stats.is_some() { STATS_OFFSET + STATS_BYTES } else if passes { u64::from(QUERIES) * 8 } else { 16 };
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("GPU time"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let ring = (0..RING)
            .map(|_| Slot {
                buf: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("GPU time readback"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(IDLE)),
                meta: None,
            })
            .collect();
        GpuTimer {
            plan: Plan::new(mode, passes),
            queries,
            stats,
            resolve,
            ring,
            next: 0,
            current: None,
            period_ns: f64::from(queue.get_timestamp_period()),
            report: Report::default(),
        }
    }

    /// Collects the measurements that arrived (non-blocking).
    pub fn collect(&mut self) {
        for slot in &mut self.ring {
            if slot.state.load(Ordering::Acquire) != READY {
                continue;
            }
            if let Some(meta) = slot.meta.take()
                && let Ok(view) = slot.buf.slice(..).get_mapped_range()
            {
                let words: &[u64] = bytemuck::cast_slice(&view);
                let ticks = &words[..meta.range.len()];
                let s = (STATS_OFFSET / 8) as usize;
                let stats = (meta.stats && words.len() >= s + 2).then(|| [words[s], words[s + 1]]);
                self.report.add(&meta, ticks, self.period_ns, stats);
            }
            slot.buf.unmap();
            slot.state.store(IDLE, Ordering::Release);
        }
    }

    /// Start of the frame: takes a free buffer (otherwise frame not measured).
    pub fn begin(&mut self, enc: &mut wgpu::CommandEncoder) {
        let i = self.next;
        if self.ring[i].state.load(Ordering::Acquire) != IDLE {
            return;
        }
        self.next = (i + 1) % self.ring.len();
        self.current = Some(i);
        self.plan.start();
        if self.plan.mode == FrameMode::Encoder {
            enc.write_timestamp(&self.queries, 0);
        }
    }

    /// Timestamps of a pass (to give to `RenderPassDescriptor::timestamp_writes`).
    pub fn pass(&self, name: &'static str, role: Role) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.plan.assign(name, role).map(|(b, e)| wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: b,
            end_of_pass_write_index: e,
        })
    }

    /// Pipeline statistics of the world pass (diagnostic), if offered.
    pub fn stats_begin(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(q) = self.stats.as_ref().filter(|_| self.plan.is_active()) {
            pass.begin_pipeline_statistics_query(q, 0);
            self.plan.mark_stats();
        }
    }

    pub fn stats_end(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.stats.is_some() && self.plan.stats.get() && self.plan.is_active() {
            pass.end_pipeline_statistics_query();
        }
    }

    /// End of the frame: final timestamp, resolution of the written queries, copy to the ring.
    pub fn end(&mut self, enc: &mut wgpu::CommandEncoder) {
        let Some(i) = self.current else { return };
        if self.plan.mode == FrameMode::Encoder {
            enc.write_timestamp(&self.queries, 1);
        }
        let meta = self.plan.finish();
        let range = meta.range.clone();
        if !range.is_empty() {
            enc.resolve_query_set(&self.queries, range.clone(), &self.resolve, 0);
            enc.copy_buffer_to_buffer(&self.resolve, 0, &self.ring[i].buf, 0, u64::from(range.len() as u32) * 8);
        }
        if meta.stats
            && let Some(q) = &self.stats
        {
            enc.resolve_query_set(q, 0..1, &self.resolve, STATS_OFFSET);
            enc.copy_buffer_to_buffer(&self.resolve, STATS_OFFSET, &self.ring[i].buf, STATS_OFFSET, STATS_BYTES);
        }
        if range.is_empty() && !meta.stats {
            self.current = None;
            return;
        }
        self.ring[i].meta = Some(meta);
    }

    /// After the submit: deferred readback of the frame's buffer.
    pub fn after_submit(&mut self) {
        let Some(i) = self.current.take() else { return };
        let state = self.ring[i].state.clone();
        state.store(PENDING, Ordering::Release);
        self.ring[i].buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            state.store(if r.is_ok() { READY } else { IDLE }, Ordering::Release);
        });
    }

    /// Mean total per frame since the last sample; with the breakdown, the log line
    /// is written at the same time (`pixels`: frame size).
    pub fn average_ms(&mut self, pixels: u64) -> Option<f64> {
        let total = self.report.take_total();
        if self.plan.passes {
            eprintln!("{}", self.report.line(total, self.plan.mode, pixels));
            self.report.reset_passes();
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulated frame: writes of the encoder and of the passes, as `GpuTimer` emits them.
    /// Returns (queries written in order, frame meta).
    fn simulate(mode: FrameMode, passes: bool, seq: &[(&'static str, Role)]) -> (Vec<u32>, Meta) {
        let plan = Plan::new(mode, passes);
        plan.start();
        let mut writes = Vec::new();
        if mode == FrameMode::Encoder {
            writes.push(0);
        }
        for &(name, role) in seq {
            if let Some((b, e)) = plan.assign(name, role) {
                writes.extend(b);
                writes.extend(e);
            }
        }
        if mode == FrameMode::Encoder {
            writes.push(1);
        }
        let meta = plan.finish();
        (writes, meta)
    }

    const MODES: [FrameMode; 3] = [FrameMode::Encoder, FrameMode::Passes, FrameMode::World];

    /// Realistic sequences of passes: without atlas or blur, with atlas, transparency and blur, and a
    /// sequence longer than `MAX_PASSES`.
    fn sequences() -> Vec<Vec<(&'static str, Role)>> {
        let mut long = vec![("item atlas", Role::Inner), ("world", Role::World)];
        long.extend((0..MAX_PASSES + 5).map(|_| ("blur", Role::Inner)));
        long.push(("interface after blur", Role::Last));
        vec![
            vec![("world", Role::World), ("hand and interface", Role::Last)],
            vec![
                ("item atlas", Role::Inner),
                ("world", Role::World),
                ("transparency: terrain, bounds", Role::Inner),
                ("transparency: composition", Role::Inner),
                ("hand and interface", Role::Inner),
                ("blur", Role::Inner),
                ("blur", Role::Inner),
                ("interface after blur", Role::Last),
            ],
            long,
        ]
    }

    /// Every query of the resolved range is written exactly once, none outside the range:
    /// on Vulkan, a query not written within the range would make the GPU wait forever.
    #[test]
    fn resolved_range_is_exactly_the_written_queries() {
        for mode in MODES {
            for passes in [false, true] {
                for seq in sequences() {
                    let (writes, meta) = simulate(mode, passes, &seq);
                    let mut sorted = writes.clone();
                    sorted.sort_unstable();
                    let expected: Vec<u32> = meta.range.clone().collect();
                    assert_eq!(sorted, expected, "{mode:?} passes={passes}: writes {writes:?}");
                    assert!(meta.range.end <= if passes { QUERIES } else { 2 });
                    // Total always defined in these sequences (last pass marked).
                    let (b, e) = meta.frame;
                    assert!(b.is_some() && e.is_some(), "{mode:?} passes={passes} : total {:?}", meta.frame);
                }
            }
        }
    }

    /// Outside a measured frame (no free buffer), no pass writes.
    #[test]
    fn inactive_frame_writes_nothing() {
        let plan = Plan::new(FrameMode::Passes, true);
        assert!(plan.assign("world", Role::World).is_none());
        plan.start();
        let _ = plan.finish();
        assert!(plan.assign("hand and interface", Role::Last).is_none());
    }

    /// Increasing fake timestamps (step of 10, passes separated by 3 of barriers): total,
    /// passes accumulated by name, sum of the passes ≤ total, "outside passes" = the difference.
    #[test]
    fn decode_sums_by_name_and_fits_in_total() {
        for mode in [FrameMode::Encoder, FrameMode::Passes] {
            let seq = &sequences()[1];
            let (_, meta) = simulate(mode, true, seq);
            // Times: encoder at 0, pass k from 5 + 13k to 15 + 13k, end of the encoder after everything.
            let mut ticks = vec![0u64; meta.range.len()];
            let mut put = |i: u32, v: u64| ticks[(i - meta.range.start) as usize] = v;
            for k in 0..meta.n {
                put(2 + 2 * k, 5 + 13 * u64::from(k));
                put(3 + 2 * k, 15 + 13 * u64::from(k));
            }
            if mode == FrameMode::Encoder {
                put(0, 0);
                put(1, 15 + 13 * u64::from(meta.n) + 7);
            }
            let mut r = Report::default();
            // Period 1e6 ns: 1 tick = 1 ms.
            r.add(&meta, &ticks, 1e6, None);
            let passes: Vec<_> = r.passes().collect();
            let total = r.take_total().expect("total");
            let sum: f64 = passes.iter().map(|p| p.1).sum();
            assert_eq!(passes.iter().find(|p| p.0 == "blur").map(|p| p.1), Some(20.0));
            assert_eq!(passes.len(), 7, "{passes:?}");
            assert_eq!(sum, 80.0);
            let expect_total = if mode == FrameMode::Encoder { 15.0 + 13.0 * 8.0 + 7.0 } else { 15.0 + 13.0 * 7.0 - 5.0 };
            assert_eq!(total, expect_total);
            assert!(sum <= total);
            let line = r.line(Some(total), mode, 1);
            assert!(line.contains(&format!("outside passes {:.3}", total - sum)), "{line}");
        }
    }

    /// Old measurement: the total is the world pass's pair, with or without breakdown.
    #[test]
    fn world_mode_total_is_the_world_pass() {
        for passes in [false, true] {
            let seq = &sequences()[1];
            let (_, meta) = simulate(FrameMode::World, passes, seq);
            let mut ticks: Vec<u64> = (0..meta.range.len() as u64).map(|i| 100 + 10 * i).collect();
            if !passes {
                ticks = vec![100, 142];
            }
            let mut r = Report::default();
            r.add(&meta, &ticks, 1e6, None);
            let total = r.take_total().expect("total");
            // With breakdown, the world pass is the second pair (indices 4, 5 → 120, 130).
            assert_eq!(total, if passes { 10.0 } else { 42.0 });
        }
    }

    /// Minimal GPU bench: 1600 × 900 frame, `PASSES` full-screen passes (somewhat heavy colour
    /// computation, blending), drawn with or without timestamps.
    struct Bench {
        device: wgpu::Device,
        queue: wgpu::Queue,
        pipeline: wgpu::RenderPipeline,
        target: wgpu::Texture,
        view: wgpu::TextureView,
    }

    const W: u32 = 1600;
    const H: u32 = 900;
    const PASSES: usize = 12;
    const NAMES: [&str; 4] = ["world", "transparency", "blur", "interface"];

    const SHADER: &str = "
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var v = pos.x * 0.013 + pos.y * 0.007;
    for (var k = 0; k < 48; k++) { v = fract(sin(v * 12.9898 + f32(k)) * 43758.547); }
    return vec4<f32>(v, 1.0 - v, fract(v * 7.0), 0.25);
}";

    impl Bench {
        fn new() -> Option<Bench> {
            crate::render::upload::tests::gpu_serial();
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }))
            .ok();
            let adapter = rc_testdata::need(adapter, "GPU adapter")?;
            let wanted = wgpu::Features::TIMESTAMP_QUERY
                | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                | wgpu::Features::PIPELINE_STATISTICS_QUERY;
            let features = adapter.features() & wanted;
            rc_testdata::need(features.contains(wgpu::Features::TIMESTAMP_QUERY).then_some(()), "GPU with timestamps")?;
            eprintln!("GPU: {} ({:?}), features {features:?}", adapter.get_info().name, adapter.get_info().backend);
            let (device, queue) = pollster::block_on(
                adapter.request_device(&wgpu::DeviceDescriptor { required_features: features, ..Default::default() }),
            )
            .expect("device");
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(SHADER.into()),
            });
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            Some(Bench { device, queue, pipeline, target, view })
        }

        fn wait(&self) {
            self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
        }

        /// One frame; with `timer`, each pass is timestamped as in the renderer.
        fn frame(&self, mut timer: Option<&mut GpuTimer>) {
            if let Some(t) = timer.as_deref_mut() {
                t.collect();
            }
            let mut enc = self.device.create_command_encoder(&Default::default());
            if let Some(t) = timer.as_deref_mut() {
                t.begin(&mut enc);
            }
            for k in 0..PASSES {
                let role = match k {
                    1 => Role::World,
                    k if k == PASSES - 1 => Role::Last,
                    _ => Role::Inner,
                };
                let t = timer.as_deref();
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if k == 0 { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load },
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    timestamp_writes: t.and_then(|t| t.pass(NAMES[k % NAMES.len()], role)),
                    ..Default::default()
                });
                if k == 1
                    && let Some(t) = t
                {
                    t.stats_begin(&mut pass);
                }
                pass.set_pipeline(&self.pipeline);
                pass.draw(0..3, 0..1);
                if k == 1
                    && let Some(t) = t
                {
                    t.stats_end(&mut pass);
                }
            }
            if let Some(t) = timer.as_deref_mut() {
                t.end(&mut enc);
            }
            self.queue.submit([enc.finish()]);
            if let Some(t) = timer {
                t.after_submit();
            }
        }

        /// Pixels of the target.
        fn pixels(&self) -> Vec<u8> {
            let row = W * 4;
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row * H),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut enc = self.device.create_command_encoder(&Default::default());
            enc.copy_texture_to_buffer(
                self.target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(H),
                    },
                },
                wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            );
            self.queue.submit([enc.finish()]);
            buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            self.wait();
            let v = buf.slice(..).get_mapped_range().expect("read").to_vec();
            buf.unmap();
            v
        }
    }

    /// Real GPU: same pixels with and without timestamps in each mode; total present, sum
    /// of the passes ≤ total, number of named passes, overdraw of the "world" pass = 1.
    #[test]
    fn gpu_same_pixels_and_sum_fits_in_total() {
        let Some(b) = Bench::new() else { return };
        b.frame(None);
        let reference = b.pixels();
        let encoder_ok = b.device.features().contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        for mode in MODES {
            if mode == FrameMode::Encoder && !encoder_ok {
                eprintln!("encoder mode not offered by the adapter: skipped");
                continue;
            }
            for passes in [false, true] {
                let mut t = GpuTimer::new(&b.device, &b.queue, mode, passes);
                for _ in 0..12 {
                    b.frame(Some(&mut t));
                    b.wait();
                }
                t.collect();
                assert_eq!(b.pixels(), reference, "{mode:?} passes={passes}: pixels changed");
                let r = &t.report;
                let frames = r.total_frames;
                assert!(frames >= 8, "{mode:?} passes={passes}: {frames} totals sampled");
                let total = r.total_ms / f64::from(frames);
                if passes {
                    let names: Vec<_> = r.passes().collect();
                    assert_eq!(names.len(), NAMES.len(), "{names:?}");
                    let sum: f64 = names.iter().map(|p| p.1).sum();
                    eprintln!("{mode:?}: total {total:.4} ms, sum of passes {sum:.4} ms, {names:?}");
                    if mode == FrameMode::World {
                        assert!(total < sum, "{mode:?}: the world pass alone must be below the sum");
                    } else {
                        assert!(sum <= total * 1.0001, "{mode:?}: sum {sum} > total {total}");
                    }
                    if r.stat_frames > 0 {
                        let overdraw = r.fragments as f64 / f64::from(r.stat_frames) / f64::from(W * H);
                        assert!((0.99..1.2).contains(&overdraw), "overdraw {overdraw}");
                        eprintln!("{mode:?}: overdraw of the 'world' pass {overdraw:.3}");
                    }
                } else {
                    eprintln!("{mode:?} without breakdown: total {total:.4} ms");
                }
            }
        }
    }

    /// Waits, without ever calling `device.poll`, for slot `i` to be read: only empty submits
    /// make wgpu advance (it collects finished work and delivers the callbacks on every submit).
    fn ready_by_submits(b: &Bench, t: &GpuTimer, i: usize) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while t.ring[i].state.load(Ordering::Acquire) != READY {
            if std::time::Instant::now() > deadline {
                return false;
            }
            b.queue.submit([]);
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        true
    }

    /// End of frame without `device.poll` (`RUSTCRAFT_OPT_NOPOLL`, `render.rs`), frame by frame, no
    /// poll anywhere (not even a final wait): imposed rhythm = frame N is rendered by the GPU before
    /// frame N+1, established by submits alone. The start-of-frame sample of frame N+1 must
    /// then contain exactly frames 0..=N, as with the end-of-frame poll. If the callbacks
    /// were delivered only by a poll, the wait fails (sabotage verified: wait by sleeping
    /// alone, without an empty submit; callback that never marks the slot as read).
    #[test]
    fn no_poll_delivers_at_next_submit() {
        let Some(b) = Bench::new() else { return };
        const FRAMES: u32 = 40;
        let mut seen: Vec<Vec<u32>> = Vec::new();
        for poll in [true, false] {
            let mut t = GpuTimer::new(&b.device, &b.queue, GpuTimer::mode_for(b.device.features()), false);
            let mut collected = Vec::new();
            for n in 0..FRAMES {
                let i = t.next;
                // The start-of-frame sample is done in `frame`; its effect is read right after.
                b.frame(Some(&mut t));
                collected.push(t.report.total_frames);
                assert_eq!(t.next, (i + 1) % RING, "poll {poll}: frame {n} not measured (ring full)");
                if poll {
                    let _ = b.device.poll(wgpu::PollType::Poll);
                }
                assert!(ready_by_submits(&b, &t, i), "poll {poll}: callback of frame {n} never delivered by a submit");
            }
            t.collect();
            collected.push(t.report.total_frames);
            seen.push(collected);
        }
        // Sample of frame N+1 = N+1 frames (0..=N), with and without poll.
        let expected: Vec<u32> = (0..=FRAMES).collect();
        assert_eq!(seen[0], expected, "with poll");
        assert_eq!(seen[1], expected, "without poll");
    }

    /// Bench (to run by hand, `--ignored`): wall time of 300 GPU-limited frames, without
    /// timestamps, total alone, total + breakdown.
    #[test]
    #[ignore = "bench"]
    fn bench_gpu_time() {
        let Some(b) = Bench::new() else { return };
        let run = |mut t: Option<GpuTimer>| {
            for _ in 0..30 {
                b.frame(t.as_mut());
            }
            b.wait();
            let t0 = std::time::Instant::now();
            for _ in 0..300 {
                b.frame(t.as_mut());
            }
            b.wait();
            t0.elapsed().as_secs_f64() * 1e3 / 300.0
        };
        for pass in 0..3 {
            let none = run(None);
            let total = run(Some(GpuTimer::new(&b.device, &b.queue, FrameMode::Encoder, false)));
            let fallback = run(Some(GpuTimer::new(&b.device, &b.queue, FrameMode::Passes, false)));
            let all = run(Some(GpuTimer::new(&b.device, &b.queue, FrameMode::Encoder, true)));
            eprintln!(
                "pass {pass}: ms/frame without {none:.4}, encoder total {total:.4}, passes total {fallback:.4}, total + {PASSES} passes {all:.4}"
            );
        }
    }

    /// Statistics: fragments per frame and overdraw relative to the pixels.
    #[test]
    fn stats_line() {
        let (_, mut meta) = simulate(FrameMode::Encoder, true, &sequences()[0]);
        meta.stats = true;
        let mut r = Report::default();
        r.add(&meta, &[0, 50, 10, 20, 30, 40], 1e6, Some([3_000_000, 2_880_000]));
        let line = r.line(Some(50.0), FrameMode::Encoder, 1_440_000);
        assert!(line.contains("2.880 M fragments, overdraw 2.00, 3.000 M triangles"), "{line}");
    }
}
