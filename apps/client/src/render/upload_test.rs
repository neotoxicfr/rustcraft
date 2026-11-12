//! Proof on a real GPU: a deterministic script of 300 frames of writes (small, overlapping,
//! contiguous runs, larger than a chunk, buffers grown by a copy in the middle of the writes like
//! `alloc_in`, a per-frame layer buffer, a direct submission in the middle, a late write after the
//! frame encoder, and check copies in each frame's encoder that read the written buffers) played by
//! the queue, the ring as it was, and every path of the module header alone and together; final
//! contents and log of the copies read back and compared with the CPU model. An omitted write and a
//! growth copy recorded after the writes make the comparison fail (verified right here).
//! Texture writes through the ring are compared byte for byte with the queue's own.
//! Without a GPU the tests skip themselves; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::per_frame::{FrameBuffer, LayerOptions};
use super::*;

/// Tests on a real GPU: only one at a time in the binary. Each creates its instance and its
/// device; sixteen at once occasionally blocked all the GPU tests (about one
/// run in five, never on a single thread). Taken by each test device creation and
/// held by the test thread until its end (libtest: one thread per test; on a single thread, already
/// held, it is not taken again).
pub(crate) fn gpu_serial() {
    use std::sync::{Mutex, MutexGuard, PoisonError};
    static L: Mutex<()> = Mutex::new(());
    thread_local! {
        static HELD: std::cell::RefCell<Option<MutexGuard<'static, ()>>> = const { std::cell::RefCell::new(None) };
    }
    HELD.with(|h| {
        let mut h = h.borrow_mut();
        if h.is_none() {
            *h = Some(L.lock().unwrap_or_else(PoisonError::into_inner));
        }
    });
}

pub(crate) fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok();
    // A software adapter (WARP, llvmpipe) is not the real GPU these proofs are about.
    let hardware = adapter.filter(|a| a.get_info().device_type != wgpu::DeviceType::Cpu);
    let adapter = rc_testdata::need(hardware, "hardware GPU adapter")?;
    // Blend of float targets requested if it exists, like the client (improved transparency).
    let required_features = adapter.features() & wgpu::Features::FLOAT32_BLENDABLE;
    let desc = wgpu::DeviceDescriptor { required_features, ..Default::default() };
    Some(pollster::block_on(adapter.request_device(&desc)).expect("device"))
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn bytes(&mut self, len: u64) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }
}

const FRAMES: usize = 300;
const PROBE: u64 = 64;
const USAGE: wgpu::BufferUsages = wgpu::BufferUsages::COPY_DST.union(wgpu::BufferUsages::COPY_SRC);
/// Smallest capacity of the script's per-frame layer buffer (the layers use 1024 or 4096).
const LAYER_MIN: u64 = 1024;

pub(crate) fn read(device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Buffer) -> Vec<u8> {
    let dst = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: src.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(src, 0, &dst, 0, src.size());
    queue.submit([enc.finish()]);
    dst.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
    let v = dst.slice(..).get_mapped_range().expect("read").to_vec();
    dst.unmap();
    v
}

fn buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: USAGE, mapped_at_creation: false })
}

/// Which paths an uploader takes (the switches of the module header), set without the environment.
#[derive(Debug, Clone, Copy)]
struct Paths {
    ring: bool,
    growth_in_transit: bool,
    adaptive_chunk: bool,
    merge_copies: bool,
    frame_writes: bool,
    texture_writes: bool,
    single_command_buffer: bool,
}

impl Paths {
    /// Everything through the queue (`RUSTCRAFT_OPT_UPLOADRING=0`).
    const QUEUE: Paths = Paths {
        ring: false,
        growth_in_transit: false,
        adaptive_chunk: false,
        merge_copies: false,
        frame_writes: false,
        texture_writes: false,
        single_command_buffer: false,
    };
    /// The ring as it was before these paths (every new switch at 0).
    const RING_BEFORE: Paths = Paths { ring: true, ..Paths::QUEUE };
    const ALL_ON: Paths = Paths {
        ring: true,
        growth_in_transit: true,
        adaptive_chunk: true,
        merge_copies: true,
        frame_writes: true,
        texture_writes: true,
        single_command_buffer: true,
    };

    fn uploader(self, device: &wgpu::Device) -> Uploader {
        let opts = Options {
            growth_in_transit: self.growth_in_transit,
            adaptive_chunk: self.adaptive_chunk,
            merge_copies: self.merge_copies,
        };
        let layer_opts = LayerOptions {
            frame_writes: self.frame_writes,
            texture_writes: self.texture_writes,
            single_command_buffer: self.single_command_buffer,
        };
        Uploader::with_options(device, self.ring, opts, layer_opts)
    }
}

/// The ring paths of the script proof, each alone then all together.
const SCRIPT_PATHS: [(&str, Paths); 7] = [
    ("ring before", Paths::RING_BEFORE),
    ("frame writes", Paths { frame_writes: true, ..Paths::RING_BEFORE }),
    ("growth in transit", Paths { growth_in_transit: true, ..Paths::RING_BEFORE }),
    ("single command buffer", Paths { single_command_buffer: true, ..Paths::RING_BEFORE }),
    ("adaptive chunk", Paths { adaptive_chunk: true, ..Paths::RING_BEFORE }),
    ("merged copies", Paths { merge_copies: true, ..Paths::RING_BEFORE }),
    ("all on", Paths::ALL_ON),
];

/// CPU model of the script: the plain targets, the per-frame layer buffer (its whole capacity),
/// the buffer written late, and the log of the check copies.
struct Model {
    targets: Vec<Vec<u8>>,
    layer: Vec<u8>,
    late: Vec<u8>,
    log: Vec<u8>,
}

/// GPU side of the script.
struct Gpu<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    up: Uploader,
    targets: Vec<wgpu::Buffer>,
    layer: FrameBuffer,
    late: wgpu::Buffer,
    log: wgpu::Buffer,
    /// Buffers only touched by the direct submission in the middle of the writes.
    scratch: [wgpu::Buffer; 2],
}

/// The script played on the model, and on the GPU when there is one.
struct Script<'a> {
    rng: Rng,
    model: Model,
    gpu: Option<Gpu<'a>>,
}

impl<'a> Script<'a> {
    fn new(gpu: Option<(&'a wgpu::Device, &'a wgpu::Queue, Paths, Sabotage)>) -> Self {
        let targets: Vec<Vec<u8>> = [4096usize, 300 << 10, 3 << 20].iter().map(|&n| vec![0; n]).collect();
        let model =
            Model { targets, layer: Vec::new(), late: vec![0; 4096], log: vec![0; FRAMES * 3 * PROBE as usize] };
        let gpu = gpu.map(|(device, queue, paths, sabotage)| {
            let mut up = paths.uploader(device);
            up.sabotage = sabotage;
            Gpu {
                device,
                queue,
                up,
                targets: model.targets.iter().map(|m| buffer(device, m.len() as u64)).collect(),
                layer: FrameBuffer::new("layer", USAGE, LAYER_MIN),
                late: buffer(device, model.late.len() as u64),
                log: buffer(device, model.log.len() as u64),
                scratch: [buffer(device, 4096), buffer(device, 4096)],
            }
        });
        Script { rng: Rng(0x9e37_79b9_7f4a_7c15), model, gpu }
    }

    /// Write `w` of the frame into target `t`; odd writes fill the transit in place.
    fn write(&mut self, t: usize, off: u64, data: &[u8], w: u64) {
        self.model.targets[t][off as usize..off as usize + data.len()].copy_from_slice(data);
        let Some(g) = self.gpu.as_mut() else { return };
        if w % 2 == 1 {
            g.up.write_with(g.queue, &g.targets[t], off, data.len() as u64, |mut v| v.copy_from_slice(data));
        } else {
            g.up.write(g.queue, &g.targets[t], off, data);
        }
    }

    /// Random writes of a frame: single ones (small, overlapping, larger than a chunk from time to
    /// time) and contiguous runs; target 0 or 1 grows once in a while between two writes.
    fn writes(&mut self, frame: usize) {
        let writes = 4 + self.rng.below(16);
        for w in 0..writes {
            let t = self.rng.below(3) as usize;
            let size = self.model.targets[t].len() as u64;
            if self.rng.below(4) == 0 {
                self.contiguous_run(t, size, w);
            } else {
                let max = if t == 2 && self.rng.below(12) == 0 { 3 * CHUNK / 2 } else { 4096 };
                let len = (4 + self.rng.below(max.min(size) - 4)) & !3;
                let off = self.rng.below(size - len) & !3;
                let data = self.rng.bytes(len);
                self.write(t, off, &data, w);
            }
            if w == 1 && frame % 50 == 7 {
                let g = self.rng.below(2) as usize;
                self.grow(g);
            }
        }
    }

    fn contiguous_run(&mut self, t: usize, size: u64, w: u64) {
        let run = 2 + self.rng.below(5);
        let mut off = self.rng.below(size - run * 256) & !3;
        for _ in 0..run {
            let len = 4 * (1 + self.rng.below(64));
            let data = self.rng.bytes(len);
            self.write(t, off, &data, w);
            off += data.len() as u64;
        }
    }

    /// Target `g` grows by a quarter, its contents copied in order with the writes, then a write
    /// across its old end: the new range may start inside the old one, like an arena whose last
    /// hole merges with the new room.
    fn grow(&mut self, g: usize) {
        let old = self.model.targets[g].len();
        let grown = (old + old / 4).next_multiple_of(4);
        self.model.targets[g].resize(grown, 0);
        if let Some(gpu) = self.gpu.as_mut() {
            let bigger = buffer(gpu.device, grown as u64);
            gpu.up.copy_buffer(gpu.queue, &gpu.targets[g], 0, &bigger, 0, old as u64);
            gpu.targets[g] = bigger;
        }
        let data = self.rng.bytes(128);
        self.write(g, old as u64 - 64, &data, 0);
    }

    /// The per-frame layer: rewritten every frame, a little longer as the frames go (it grows).
    fn layer(&mut self, frame: usize) {
        let max = 256u64 << (frame / 50).min(8);
        let len = (PROBE + self.rng.below(max)) & !3;
        let data = self.rng.bytes(len);
        if (self.model.layer.len() as u64) < len {
            self.model.layer = vec![0; len.next_power_of_two().max(LAYER_MIN) as usize];
        }
        self.model.layer[..data.len()].copy_from_slice(&data);
        if let Some(g) = self.gpu.as_mut() {
            if frame.is_multiple_of(2) {
                g.layer.write(&mut g.up, g.queue, &data);
            } else {
                g.layer.write_with(&mut g.up, g.queue, len, |mut v| v.copy_from_slice(&data));
            }
        }
    }

    /// A submission of its own in the middle of the writes, reading nothing the ring writes.
    fn direct_submission(&mut self) {
        let Some(g) = self.gpu.as_mut() else { return };
        let mut enc = g.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&g.scratch[0], 0, &g.scratch[1], 0, 4096);
        g.queue.submit([enc.finish()]);
    }

    /// End of the frame: the frame encoder, a late write on one frame in four, check copies of the
    /// three written buffers into the log, submission.
    fn end_frame(&mut self, frame: usize) {
        let enc = self.gpu.as_mut().map(|g| g.up.frame_encoder());
        if frame.is_multiple_of(4) {
            let len = (4 + self.rng.below(256)) & !3;
            let off = self.rng.below(self.model.late.len() as u64 - len) & !3;
            let data = self.rng.bytes(len);
            self.model.late[off as usize..(off + len) as usize].copy_from_slice(&data);
            if let Some(g) = self.gpu.as_mut() {
                g.up.write(g.queue, &g.late, off, &data);
            }
        }
        let at = self.rng.below(self.model.targets[0].len() as u64 - PROBE) & !3;
        let late_at = self.rng.below(self.model.late.len() as u64 - PROBE) & !3;
        let row = frame * 3 * PROBE as usize;
        let p = PROBE as usize;
        self.model.log[row..row + p].copy_from_slice(&self.model.targets[0][at as usize..at as usize + p]);
        self.model.log[row + p..row + 2 * p].copy_from_slice(&self.model.layer[..p]);
        self.model.log[row + 2 * p..row + 3 * p]
            .copy_from_slice(&self.model.late[late_at as usize..late_at as usize + p]);
        let (Some(g), Some(mut enc)) = (self.gpu.as_mut(), enc) else { return };
        let row = row as u64;
        enc.copy_buffer_to_buffer(&g.targets[0], at, &g.log, row, PROBE);
        enc.copy_buffer_to_buffer(g.layer.buffer().expect("written"), 0, &g.log, row + PROBE, PROBE);
        enc.copy_buffer_to_buffer(&g.late, late_at, &g.log, row + 2 * PROBE, PROBE);
        g.up.submit(g.queue, enc.finish());
        if frame % 3 == 2 {
            // Rhythm of a frame loop (a few frames in flight).
            let _ = g.device.poll(wgpu::PollType::Poll);
        }
    }

    /// Final contents: the targets, the layer buffer, the late buffer and the log.
    fn finish(self) -> (Vec<Vec<u8>>, Stats) {
        match self.gpu {
            Some(mut g) => {
                let mut out: Vec<Vec<u8>> = g.targets.iter().map(|b| read(g.device, g.queue, b)).collect();
                out.push(read(g.device, g.queue, g.layer.buffer().expect("written")));
                out.push(read(g.device, g.queue, &g.late));
                out.push(read(g.device, g.queue, &g.log));
                (out, g.up.take_stats())
            }
            None => {
                let mut out = self.model.targets;
                out.extend([self.model.layer, self.model.late, self.model.log]);
                (out, Stats::default())
            }
        }
    }
}

/// Plays the script on the GPU (`gpu` = Some) or on the CPU model; returns the final contents and
/// the counters.
fn play(gpu: Option<(&wgpu::Device, &wgpu::Queue, Paths, Sabotage)>) -> (Vec<Vec<u8>>, Stats) {
    let mut s = Script::new(gpu);
    for frame in 0..FRAMES {
        s.writes(frame);
        s.layer(frame);
        if frame % 10 == 3 {
            s.direct_submission();
        }
        s.end_frame(frame);
    }
    s.finish()
}

/// First buffer and byte that differ, if any.
fn first_difference(got: &[Vec<u8>], want: &[Vec<u8>]) -> Option<(usize, usize)> {
    if got.len() != want.len() {
        return Some((got.len().min(want.len()), 0));
    }
    got.iter().zip(want).enumerate().find_map(|(i, (g, w))| {
        let byte = g.iter().zip(w).position(|(a, b)| a != b);
        byte.or_else(|| (g.len() != w.len()).then_some(g.len().min(w.len()))).map(|b| (i, b))
    })
}

fn digest(v: &[Vec<u8>]) -> u64 {
    v.iter().flatten().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

#[test]
fn ring_writes_match_queue_writes() {
    let Some((device, queue)) = gpu() else { return };
    let (expected, _) = play(None);
    let (old, old_stats) = play(Some((&device, &queue, Paths::QUEUE, Sabotage::default())));
    eprintln!("queue: {old_stats:?}; fingerprint {:016x}", digest(&expected));
    assert_eq!(first_difference(&old, &expected), None, "queue path differs from the model (buffer, byte)");
    assert_eq!(old_stats.transits, old_stats.writes, "queue path: one transit per write");
    let mut stats = Vec::new();
    for (name, paths) in SCRIPT_PATHS {
        let (got, s) = play(Some((&device, &queue, paths, Sabotage::default())));
        eprintln!("{name}: {s:?}");
        assert_eq!(first_difference(&got, &expected), None, "{name} differs from the model (buffer, byte)");
        assert_eq!(s.writes, old_stats.writes, "{name}: same writes");
        stats.push(s);
    }
    let [before, _, growth, single, adaptive, merged, all] = stats.try_into().expect("seven paths");
    assert!(before.transits * 4 < before.writes, "ring: chunks recycled ({before:?})");
    assert!(adaptive.transits < before.transits, "adaptive chunks: fewer disposable transits");
    assert!(merged.copies < before.copies, "merged copies: fewer GPU copies");
    assert!(growth.command_buffers < before.command_buffers, "growth in transit: fewer command buffers");
    assert!(single.command_buffers < before.command_buffers, "single command buffer: fewer command buffers");
    assert!(all.command_buffers < single.command_buffers.min(growth.command_buffers));
    // Sabotages: an omitted write, and growth copies recorded after the writes, must show.
    let omitted = Sabotage { omit: Some(all.writes - 1), ..Default::default() };
    let (bad, _) = play(Some((&device, &queue, Paths::ALL_ON, omitted)));
    assert!(first_difference(&bad, &expected).is_some(), "omitted write not detected");
    let late_growth = Sabotage { growth_copies_last: true, ..Default::default() };
    let (bad, _) = play(Some((&device, &queue, Paths::ALL_ON, late_growth)));
    assert!(first_difference(&bad, &expected).is_some(), "growth copy recorded after the writes not detected");
}

/// The region buffers' real growth (`alloc_in`): random allocations written at once, releases (the
/// arena merges a released last range with the new room, so a new range may start inside the old
/// buffer), growth copies submitted at once or recorded in the transit; the buffer read back equals
/// the model, and a growth copy recorded after the writes is caught.
fn play_region_growth(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    paths: Paths,
    sabotage: Sabotage,
) -> (Vec<u8>, Vec<u8>) {
    const STRIDE: u64 = 16;
    let mut rng = Rng(99);
    let mut up = paths.uploader(device);
    up.sabotage = sabotage;
    let mut arena = crate::arena::Arena::new(64);
    let mut buf = buffer(device, 64 * STRIDE);
    let mut model = vec![0u8; (64 * STRIDE) as usize];
    let mut live: Vec<(u32, u32)> = Vec::new();
    for step in 0..400 {
        if !live.is_empty() && rng.below(3) == 0 {
            // Most often the last allocation: its range then merges with the room of a growth.
            let i = if rng.below(2) == 0 { live.len() - 1 } else { rng.below(live.len() as u64) as usize };
            let (start, n) = live.swap_remove(i);
            arena.release(start, n);
        } else {
            let n = 1 + rng.below(40) as u32;
            let start = super::super::regions::alloc_in(device, queue, &mut up, &mut buf, &mut arena, n, STRIDE);
            model.resize((u64::from(arena.capacity) * STRIDE) as usize, 0);
            let data = rng.bytes(u64::from(n) * STRIDE);
            let at = u64::from(start) * STRIDE;
            model[at as usize..at as usize + data.len()].copy_from_slice(&data);
            up.write(queue, &buf, at, &data);
            live.push((start, n));
        }
        if step % 5 == 4 {
            up.submit(queue, device.create_command_encoder(&Default::default()).finish());
        }
    }
    up.submit(queue, device.create_command_encoder(&Default::default()).finish());
    (read(device, queue, &buf), model)
}

#[test]
fn region_growth_keeps_contents() {
    let Some((device, queue)) = gpu() else { return };
    let growth = Paths { growth_in_transit: true, ..Paths::RING_BEFORE };
    for (name, paths) in [("queue", Paths::QUEUE), ("ring before", Paths::RING_BEFORE), ("growth in transit", growth)] {
        let (got, model) = play_region_growth(&device, &queue, paths, Sabotage::default());
        let first = got.iter().zip(&model).position(|(a, b)| a != b);
        assert!(got.len() == model.len() && first.is_none(), "{name}: region differs at byte {first:?}");
    }
    let late = Sabotage { growth_copies_last: true, ..Default::default() };
    let (bad, model) = play_region_growth(&device, &queue, growth, late);
    assert!(bad != model, "growth copy recorded after the writes not detected");
}

/// Without any `device.poll` (`RUSTCRAFT_OPT_NOPOLL`): chunks come back through submissions
/// alone (wgpu delivers the mapping callbacks on every submit). Imposed rhythm: each
/// frame waits, through empty submissions, for its chunk to come back; a callback delivered only by
/// a poll would make the wait fail (and create a new chunk per frame).
#[test]
fn chunks_come_back_without_poll() {
    let Some((device, queue)) = gpu() else { return };
    let target = buffer(&device, 1 << 16);
    for paths in [Paths::RING_BEFORE, Paths::ALL_ON] {
        let mut up = paths.uploader(&device);
        let data = vec![5u8; 4096];
        for frame in 0..60 {
            up.write(&queue, &target, 0, &data);
            let enc = up.frame_encoder();
            up.submit(&queue, enc.finish());
            // Waits for the chunk itself; 120 s is only an anti-hang guard (a chunk that only a
            // poll brings back never comes).
            let guard = std::time::Instant::now() + std::time::Duration::from_secs(120);
            while up.back_rx.is_empty() {
                assert!(
                    std::time::Instant::now() < guard,
                    "frame {frame}: chunk never returned without poll (120 s anti-hang guard)"
                );
                queue.submit([]);
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        }
        let s = up.take_stats();
        assert_eq!((s.writes, s.transits), (60, 1), "{paths:?}: a single chunk, recycled on every frame");
    }
}

/// One run of the slot script: grouped slots, skipped unchanged values, paths of the uploader
/// (the per-frame uniform takes the ring or the queue), omitted write.
struct SlotRun {
    merged: bool,
    skip: bool,
    paths: Paths,
    omit: Option<u64>,
}

/// Plays 300 frames of slots plus a per-frame uniform (CPU model if `gpu` = None); returns the log
/// (all the slots and the uniform read back by a copy in the encoder of EVERY frame, hence what its
/// draws see) and the number of writes.
fn play_slots(gpu: Option<(&wgpu::Device, &wgpu::Queue, SlotRun)>) -> (Vec<u8>, u64) {
    const N: usize = 4;
    const ROW: u64 = ((N + 1) * SLOT) as u64;
    let mut rng = Rng(42);
    let mut model = [[0u8; SLOT]; N];
    let mut uniform = [0u8; SLOT];
    let mut journal = Vec::with_capacity(FRAMES * ROW as usize);
    let mut dev = gpu.map(|(device, queue, run)| {
        let mut up = run.paths.uploader(device);
        up.sabotage.omit = run.omit;
        let slots = Slots::<N>::new(device, ["a", "b", "c", "d"], run.merged, run.skip);
        let layer = FrameBuffer::new("uniform", USAGE, SLOT as u64);
        (device, queue, up, slots, layer, buffer(device, FRAMES as u64 * ROW))
    });
    // A few possible values per slot: the repetitions exercise the skipping.
    let pool: Vec<[u8; SLOT]> = (0..3).map(|_| std::array::from_fn(|_| rng.next() as u8)).collect();
    for frame in 0..FRAMES {
        for (i, slot) in model.iter_mut().enumerate() {
            if rng.below(3) != 0 {
                let v = pool[rng.below(pool.len() as u64) as usize];
                *slot = v;
                if let Some((_, _, _, s, _, _)) = dev.as_mut() {
                    s.set(i, &v);
                }
            }
        }
        let rewrite = frame == 0 || rng.below(3) != 0;
        if rewrite {
            uniform = pool[rng.below(pool.len() as u64) as usize];
        }
        journal.extend_from_slice(model.as_flattened());
        journal.extend_from_slice(&uniform);
        if let Some((_, queue, up, s, layer, log)) = dev.as_mut() {
            s.flush(up, queue);
            if rewrite {
                layer.write(up, queue, &uniform);
            }
            let mut enc = up.frame_encoder();
            let base = frame as u64 * ROW;
            if s.merged {
                enc.copy_buffer_to_buffer(&s.bufs[0], 0, log, base, (N * SLOT) as u64);
            } else {
                for (i, b) in s.bufs.iter().enumerate() {
                    enc.copy_buffer_to_buffer(b, 0, log, base + (i * SLOT) as u64, SLOT as u64);
                }
            }
            let uniform_at = base + (N * SLOT) as u64;
            enc.copy_buffer_to_buffer(layer.buffer().expect("written at frame 0"), 0, log, uniform_at, SLOT as u64);
            up.submit(queue, enc.finish());
        }
    }
    match dev {
        Some((device, queue, mut up, _, _, log)) => (read(device, queue, &log), up.take_stats().writes),
        None => (journal, 0),
    }
}

#[test]
fn slots_match_separate_buffers() {
    let Some((device, queue)) = gpu() else { return };
    let (expected, _) = play_slots(None);
    let row = 5 * SLOT;
    let mut writes = Vec::new();
    let runs = [(false, false), (false, true), (true, false), (true, true)];
    for (paths, label) in [(Paths::RING_BEFORE, "uniform by the queue"), (Paths::ALL_ON, "uniform by the ring")] {
        for (merged, skip) in runs {
            let run = SlotRun { merged, skip, paths, omit: None };
            let (got, w) = play_slots(Some((&device, &queue, run)));
            // Frame-by-frame comparison: the first wrong frame is named.
            let bad = got.chunks(row).zip(expected.chunks(row)).position(|(g, e)| g != e);
            assert!(
                got.len() == expected.len() && bad.is_none(),
                "{label}, merged {merged}, skip {skip}: frame {bad:?}"
            );
            writes.push(w);
        }
    }
    eprintln!("writes (separate, separate+skip, merged, merged+skip; queue then ring uniform): {writes:?}");
    assert!(writes[3] < writes[1] && writes[1] < writes[0] && writes[2] < writes[0]);
    // Transient sabotage: a write omitted in the middle (slot wrong for a few frames, then
    // rewritten) must show in the log, even if the final state is right.
    let run = SlotRun { merged: true, skip: true, paths: Paths::ALL_ON, omit: Some(writes[7] / 2) };
    let (bad, _) = play_slots(Some((&device, &queue, run)));
    let first = bad.chunks(row).zip(expected.chunks(row)).position(|(g, e)| g != e);
    let last_ok = bad[bad.len() - row..] == expected[expected.len() - row..];
    eprintln!("sabotage: first wrong frame {first:?}, last correct frame {last_ok}");
    assert!(first.is_some(), "omitted write not detected");
}

/// Textures of the texture-write proof: a light map (16 × 16, rows of 64 bytes padded to 256),
/// an odd-sized 3-layer array and a 2-layer texture with two mips (rows already at 256 bytes).
fn proof_textures(device: &wgpu::Device) -> [wgpu::Texture; 3] {
    let texture = |width, height, layers, mips| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width, height, depth_or_array_layers: layers },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    [texture(16, 16, 1, 1), texture(7, 5, 3, 1), texture(64, 64, 2, 2)]
}

/// Every texel of every mip and layer of `tex`, rows without padding.
fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let mut out = Vec::new();
    for mip in 0..tex.mip_level_count() {
        let size = tex.size().mip_level_size(mip, tex.dimension());
        let row = u64::from(size.width) * 4;
        let padded = row.next_multiple_of(u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT));
        let rows = u64::from(size.height) * u64::from(size.depth_or_array_layers);
        let dst = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: padded * rows,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        let layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded as u32),
            rows_per_image: Some(size.height),
        };
        let source = wgpu::TexelCopyTextureInfo { mip_level: mip, ..tex.as_image_copy() };
        enc.copy_texture_to_buffer(source, wgpu::TexelCopyBufferInfo { buffer: &dst, layout }, size);
        queue.submit([enc.finish()]);
        dst.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
        let view = dst.slice(..).get_mapped_range().expect("read");
        out.extend(view.chunks(padded as usize).flat_map(|r| &r[..row as usize]));
        drop(view);
        dst.unmap();
    }
    out
}

/// One random texture write: a region of a mip and layers, from a caller layout with an offset
/// and a stride larger than the rows.
fn random_texture_write<'t>(
    rng: &mut Rng,
    tex: &'t wgpu::Texture,
) -> (wgpu::TexelCopyTextureInfo<'t>, Vec<u8>, wgpu::TexelCopyBufferLayout, wgpu::Extent3d) {
    let mip = rng.below(u64::from(tex.mip_level_count())) as u32;
    let full = tex.size().mip_level_size(mip, tex.dimension());
    let pick = |rng: &mut Rng, n: u32| {
        let at = rng.below(u64::from(n)) as u32;
        (at, 1 + rng.below(u64::from(n - at)) as u32)
    };
    let ((x, width), (y, height), (z, layers)) =
        (pick(rng, full.width), pick(rng, full.height), pick(rng, full.depth_or_array_layers));
    let stride = width * 4 + 4 * rng.below(3) as u32;
    let rows_per_image = height + rng.below(2) as u32;
    let offset = 4 * rng.below(8);
    let len = offset + u64::from(stride) * u64::from(rows_per_image) * u64::from(layers);
    let info = wgpu::TexelCopyTextureInfo {
        texture: tex,
        mip_level: mip,
        origin: wgpu::Origin3d { x, y, z },
        aspect: wgpu::TextureAspect::All,
    };
    let layout =
        wgpu::TexelCopyBufferLayout { offset, bytes_per_row: Some(stride), rows_per_image: Some(rows_per_image) };
    (info, rng.bytes(len), layout, wgpu::Extent3d { width, height, depth_or_array_layers: layers })
}

/// 50 frames of texture writes (the whole light map, then random regions of the three textures)
/// through `paths`; the light map is copied into a log in every frame's encoder. Returns every
/// texture read back, the log, and the counters.
fn play_textures(device: &wgpu::Device, queue: &wgpu::Queue, paths: Paths, unpadded_rows: bool) -> (Vec<u8>, Stats) {
    const LIGHT_ROW: u64 = 256;
    let mut rng = Rng(7);
    let mut up = paths.uploader(device);
    up.sabotage.unpadded_rows = unpadded_rows;
    let textures = proof_textures(device);
    let log = buffer(device, 50 * 16 * LIGHT_ROW);
    for frame in 0..50u64 {
        let light = rng.bytes(16 * 16 * 4);
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(64), rows_per_image: None };
        let size = wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 };
        up.write_texture(queue, textures[0].as_image_copy(), &light, layout, size);
        for _ in 0..1 + rng.below(4) {
            let t = rng.below(3) as usize;
            let (info, data, layout, size) = random_texture_write(&mut rng, &textures[t]);
            up.write_texture(queue, info, &data, layout, size);
        }
        let mut enc = up.frame_encoder();
        let layout = wgpu::TexelCopyBufferLayout {
            offset: frame * 16 * LIGHT_ROW,
            bytes_per_row: Some(LIGHT_ROW as u32),
            rows_per_image: None,
        };
        let size = wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 };
        enc.copy_texture_to_buffer(
            textures[0].as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &log, layout },
            size,
        );
        up.submit(queue, enc.finish());
    }
    let mut out: Vec<u8> = textures.iter().flat_map(|t| read_texture(device, queue, t)).collect();
    out.extend(read(device, queue, &log));
    (out, up.take_stats())
}

#[test]
fn ring_texture_writes_match_queue_texture_writes() {
    let Some((device, queue)) = gpu() else { return };
    let (expected, queue_stats) = play_textures(&device, &queue, Paths::RING_BEFORE, false);
    assert!(expected.iter().any(|&b| b != 0), "textures written");
    let (got, ring_stats) = play_textures(&device, &queue, Paths::ALL_ON, false);
    eprintln!("queue texture writes: {queue_stats:?}; ring: {ring_stats:?}");
    let first = got.iter().zip(&expected).position(|(a, b)| a != b);
    assert!(got.len() == expected.len() && first.is_none(), "ring texture writes differ at byte {first:?}");
    assert_eq!(queue_stats.transits, queue_stats.writes, "queue path: one transit per texture write");
    assert!(ring_stats.transits * 4 < ring_stats.writes, "ring: chunks recycled ({ring_stats:?})");
    // Sabotage: rows staged without their 256-byte padding must show.
    let (bad, _) = play_textures(&device, &queue, Paths::ALL_ON, true);
    assert!(bad != expected, "missing row padding not detected");
}

/// The rules of use are enforced (debug builds and tests): a late write to a buffer already written
/// at the head of the frame encoder, and a queue write after a ring write of the same buffer, are
/// refused; the same late write is fine when the frame encoder holds no writes.
#[test]
fn rules_of_use_are_checked() {
    let Some((device, queue)) = gpu() else { return };
    let target = buffer(&device, 4096);
    let mut up = Paths::ALL_ON.uploader(&device);
    up.write(&queue, &target, 0, &[1; 16]);
    let _enc = up.frame_encoder();
    let late = catch_unwind(AssertUnwindSafe(|| up.write(&queue, &target, 16, &[2; 16])));
    assert!(late.is_err(), "late write to a buffer written at the head of the frame encoder accepted");
    let mut up = Paths::ALL_ON.uploader(&device);
    let enc = up.frame_encoder();
    up.write(&queue, &target, 16, &[2; 16]);
    up.submit(&queue, enc.finish());
    let mut up = Paths::RING_BEFORE.uploader(&device);
    up.write(&queue, &target, 0, &[1; 16]);
    let mixed = catch_unwind(AssertUnwindSafe(|| up.write_layer(&queue, &target, &[2; 16])));
    assert!(mixed.is_err(), "queue write after a ring write of the same buffer accepted");
}

/// What a bench frame writes.
#[derive(Clone, Copy)]
enum BenchLoad {
    /// Writes of these sizes, each at its own 256-byte aligned place.
    Spaced(&'static [u64]),
    /// Writes of these sizes, each right after the previous one.
    Contiguous(&'static [u64]),
    /// Writes of these sizes (spaced), then the light map: 16 × 16 texels of 4 bytes.
    LightMap(&'static [u64]),
}

/// A kind of bench frame and the paths compared on it.
type BenchSet = (&'static str, BenchLoad, &'static [(&'static str, Paths)]);

/// Frames of a bench run: writes and submission, µs per frame (total, writes, submit) and counters.
fn bench_run(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    paths: Paths,
    load: BenchLoad,
    frames: u32,
) -> (f64, f64, f64, Stats) {
    let target = buffer(device, 4 << 20);
    let [light, ..] = proof_textures(device);
    let data = vec![7u8; 2 << 20];
    let mut up = paths.uploader(device);
    let t = std::time::Instant::now();
    let (mut write_ns, mut submit_ns) = (0u128, 0u128);
    for f in 0..frames {
        let tw = std::time::Instant::now();
        let (BenchLoad::Spaced(sizes) | BenchLoad::Contiguous(sizes) | BenchLoad::LightMap(sizes)) = load;
        let contiguous = matches!(load, BenchLoad::Contiguous(_));
        let mut at = 0;
        for &s in sizes {
            up.write(queue, &target, at, &data[..s as usize]);
            at += if contiguous { s } else { s.next_multiple_of(256) };
        }
        if let BenchLoad::LightMap(_) = load {
            let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(64), rows_per_image: None };
            let size = wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 };
            up.write_texture(queue, light.as_image_copy(), &data[..1024], layout, size);
        }
        write_ns += tw.elapsed().as_nanos();
        let enc = up.frame_encoder();
        let ts = std::time::Instant::now();
        let idx = up.submit(queue, enc.finish());
        submit_ns += ts.elapsed().as_nanos();
        if f % 3 == 2 {
            let _ = device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None });
        }
    }
    let us = |ns: u128| ns as f64 / 1e3 / f64::from(frames);
    (t.elapsed().as_secs_f64() * 1e6 / f64::from(frames), us(write_ns), us(submit_ns), up.take_stats())
}

/// Micro-bench (main thread, single GPU queue: no multi-core version), 3 passes: each path of the
/// module header against the queue and the ring before it, on its own kind of frame. The rule
/// checks of the test build cost about 50 ns per ring write (well under the noise).
/// `cargo test --release -p rc-app --bin rustcraft upload_bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn upload_bench() {
    let Some((device, queue)) = gpu() else { return };
    // Typical frame (cameras, 64 KiB fade, orders, indirect commands) and small writes alone.
    const FRAME: &[u64] = &[256, 256, 256, 256, 64 << 10, 4096, 2048, 512, 512, 512, 512, 512, 8192, 8, 1024];
    const SMALL: &[u64] = &[256; 15];
    // A crowd's part matrices (more than a chunk) plus the small writes of a frame.
    const LARGE: &[u64] = &[1_200_000, 256, 256, 256, 256, 4096, 2048, 512, 512, 512, 512, 512, 8192, 8, 1024];
    const SINGLE: Paths = Paths { single_command_buffer: true, ..Paths::RING_BEFORE };
    const MERGED: Paths = Paths { merge_copies: true, ..Paths::RING_BEFORE };
    const ADAPTIVE: Paths = Paths { adaptive_chunk: true, ..Paths::RING_BEFORE };
    const TEXTURES: Paths = Paths { texture_writes: true, ..Paths::RING_BEFORE };
    const EVERY_PATH: &[(&str, Paths)] =
        &[("queue", Paths::QUEUE), ("ring", Paths::RING_BEFORE), ("single cb", SINGLE), ("all on", Paths::ALL_ON)];
    const TEXTURE_PATHS: &[(&str, Paths)] = &[("queue", Paths::RING_BEFORE), ("ring", TEXTURES)];
    let runs: [BenchSet; 6] = [
        ("frame", BenchLoad::Spaced(FRAME), EVERY_PATH),
        ("small", BenchLoad::Spaced(SMALL), EVERY_PATH),
        ("contiguous", BenchLoad::Contiguous(SMALL), &[("ring", Paths::RING_BEFORE), ("merged", MERGED)]),
        (
            "large",
            BenchLoad::Spaced(LARGE),
            &[("queue", Paths::QUEUE), ("ring", Paths::RING_BEFORE), ("adaptive", ADAPTIVE)],
        ),
        // Alone, the ring adds the transit's command buffer; in a frame, that buffer is there anyway.
        ("light map alone", BenchLoad::LightMap(&[]), TEXTURE_PATHS),
        ("light map in a frame", BenchLoad::LightMap(FRAME), TEXTURE_PATHS),
    ];
    for pass in 0..3 {
        for (set, load, paths) in runs {
            for &(name, p) in paths {
                let (total, writes, submit, stats) = bench_run(&device, &queue, p, load, 3000);
                eprintln!(
                    "pass {pass} {set} {name}: {total:.1} µs/frame (writes {writes:.1} µs, submit {submit:.1} µs), {stats:?}"
                );
            }
        }
    }
}
