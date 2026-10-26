//! Uploads to the GPU buffers, common to the whole renderer.
//!
//! **Upload transit** (`RUSTCRAFT_OPT_UPLOADRING`, default 1). Old path (`=0`): each
//! write goes through wgpu's queue write, which creates a transit buffer, fills it, then
//! destroys it after the submission (read in wgpu-core 30.0.1: one buffer per call). New
//! path: a ring of chunks mapped for writing, recycled when the GPU has finished reading them;
//! each write = one memory copy into the active chunk and one GPU copy recorded in a separate
//! transit encoder, filled in call order. At submission ([`Uploader::submit`])
//! the transit goes before the caller's commands: same semantics as the queue write
//! (writes before all the command buffers of the submit, last write of a byte wins),
//! whatever the moment of the write (before the frame's encoder or while it is being filled).
//!
//! Rules of use:
//! - every submission that follows writes goes through [`Uploader::submit`] (otherwise the writes
//!   wait for the next submission); no copy is inserted in the caller's encoder;
//! - a buffer written here is never also written by `queue.write_buffer` (within one submit, the
//!   queue writes would go before the transit and the order of the calls would be lost);
//! - a write larger than a chunk has its own disposable buffer (same cost as before);
//!   beyond `MAX_FREE` free chunks, the chunks returned are destroyed (bounded memory).
//!
//! **Frame uniforms** ([`Slots`], `RUSTCRAFT_OPT_FRAMEUNIF`, `RUSTCRAFT_OPT_UNIFSKIP`):
//! N values of 256 bytes (one camera per slot) in a single buffer with fixed offsets, one binding
//! per slot, a single write of the contiguous range of the changed slots; a value identical to
//! the last one written is not rewritten. Old path: one buffer per value, rewritten on every
//! frame. The slot buffers are never recreated (a surface resume keeps them); a
//! subsystem that recreated its own must forget the last values written (`sent`).

use crossbeam_channel::{Receiver, Sender};

/// Size of a ring chunk; a larger write has its disposable buffer.
const CHUNK: u64 = 1 << 20;
/// Free chunks kept at most (beyond, given back to the driver).
const MAX_FREE: usize = 8;
/// Size of a uniform slot (maximum alignment of the uniform binding offsets).
pub const SLOT: usize = 256;

/// Transit ring active (`RUSTCRAFT_OPT_UPLOADRING`, default 1), read once.
pub fn ring_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_UPLOADRING") != Some("0")
}

/// Frame uniforms grouped (`RUSTCRAFT_OPT_FRAMEUNIF`, default 1), read once.
pub fn merged_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_FRAMEUNIF") != Some("0")
}

/// Writes of unchanged uniforms skipped (`RUSTCRAFT_OPT_UNIFSKIP`, default 1), read once.
pub fn skip_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_UNIFSKIP") != Some("0")
}

/// Counters since the last sample: writes, transit buffers created, bytes written.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub writes: u64,
    pub transits: u64,
    pub bytes: u64,
}

/// Chunk being filled: its mapped view (kept while it is active) and the cursor.
struct Active {
    buf: wgpu::Buffer,
    view: wgpu::BufferViewMut,
    at: u64,
}

pub struct Uploader {
    device: wgpu::Device,
    ring: bool,
    enc: Option<wgpu::CommandEncoder>,
    active: Option<Active>,
    /// Chunks written since the last submission (unmapped), returned after being read.
    used: Vec<wgpu::Buffer>,
    /// Chunks back from the GPU, mapped and empty.
    free: Vec<wgpu::Buffer>,
    back_tx: Sender<wgpu::Buffer>,
    back_rx: Receiver<wgpu::Buffer>,
    stats: Stats,
    /// Test sabotage: number of the omitted write.
    #[cfg(test)]
    omit: Option<u64>,
}

impl Uploader {
    pub fn new(device: &wgpu::Device, ring: bool) -> Self {
        let (back_tx, back_rx) = crossbeam_channel::unbounded();
        Uploader {
            device: device.clone(),
            ring,
            enc: None,
            active: None,
            used: Vec::new(),
            free: Vec::new(),
            back_tx,
            back_rx,
            stats: Stats::default(),
            #[cfg(test)]
            omit: None,
        }
    }

    /// Writes `data` into `target` at `offset` (multiples of 4), effective at the next submission.
    pub fn write(&mut self, queue: &wgpu::Queue, target: &wgpu::Buffer, offset: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        #[cfg(test)]
        if self.omit == Some(self.stats.writes) {
            self.stats.writes += 1;
            return;
        }
        let len = data.len() as u64;
        self.stats.writes += 1;
        self.stats.bytes += len;
        if !self.ring {
            self.stats.transits += 1;
            queue.write_buffer(target, offset, data);
            return;
        }
        debug_assert!(
            len.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) && offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
        );
        if len > CHUNK {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("transit (large upload)"),
                size: len,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            });
            buf.slice(..).get_mapped_range_mut().expect("transit mapped at creation").copy_from_slice(data);
            buf.unmap();
            self.stats.transits += 1;
            // The buffer stays alive as long as the recorded copy has not been executed.
            self.encoder().copy_buffer_to_buffer(&buf, 0, target, offset, len);
            return;
        }
        if self.active.as_ref().is_none_or(|a| a.at + len > CHUNK) {
            self.close_active();
            let buf = self.take_chunk();
            let view = buf.slice(..).get_mapped_range_mut().expect("transit chunk mapped");
            self.active = Some(Active { buf, view, at: 0 });
        }
        let device = &self.device;
        let enc = self.enc.get_or_insert_with(|| new_encoder(device));
        let a = self.active.as_mut().expect("active chunk");
        let at = a.at;
        a.view.slice(at as usize..(at + len) as usize).copy_from_slice(data);
        a.at = at + len;
        enc.copy_buffer_to_buffer(&a.buf, at, target, offset, len);
    }

    /// Like [`Uploader::write`], the `len` bytes being filled by `fill` directly in the
    /// transit (no intermediate copy); `fill` writes the whole view received. Without ring: wgpu's
    /// queue write view.
    pub fn write_with(
        &mut self,
        queue: &wgpu::Queue,
        target: &wgpu::Buffer,
        offset: u64,
        len: u64,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
    ) {
        let Some(size) = wgpu::BufferSize::new(len) else { return };
        self.stats.writes += 1;
        self.stats.bytes += len;
        if !self.ring {
            self.stats.transits += 1;
            let mut view = queue.write_buffer_with(target, offset, size).expect("valid queue write");
            fill(view.slice(..));
            return;
        }
        debug_assert!(
            len.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) && offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
        );
        if len > CHUNK {
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("transit (large upload)"),
                size: len,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            });
            fill(buf.slice(..).get_mapped_range_mut().expect("transit mapped at creation").slice(..));
            buf.unmap();
            self.stats.transits += 1;
            self.encoder().copy_buffer_to_buffer(&buf, 0, target, offset, len);
            return;
        }
        if self.active.as_ref().is_none_or(|a| a.at + len > CHUNK) {
            self.close_active();
            let buf = self.take_chunk();
            let view = buf.slice(..).get_mapped_range_mut().expect("transit chunk mapped");
            self.active = Some(Active { buf, view, at: 0 });
        }
        let device = &self.device;
        let enc = self.enc.get_or_insert_with(|| new_encoder(device));
        let a = self.active.as_mut().expect("active chunk");
        let at = a.at;
        fill(a.view.slice(at as usize..(at + len) as usize));
        a.at = at + len;
        enc.copy_buffer_to_buffer(&a.buf, at, target, offset, len);
    }

    /// Submits `cmd` preceded by the transit of the pending writes.
    pub fn submit(&mut self, queue: &wgpu::Queue, cmd: wgpu::CommandBuffer) -> wgpu::SubmissionIndex {
        match self.finish() {
            Some(transit) => queue.submit([transit, cmd]),
            None => queue.submit([cmd]),
        }
    }

    /// Counters since the last call.
    pub fn take_stats(&mut self) -> Stats {
        std::mem::take(&mut self.stats)
    }

    fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        let device = &self.device;
        self.enc.get_or_insert_with(|| new_encoder(device))
    }

    /// Closes the transit: chunks unmapped, returned to the ring when the GPU has read them.
    fn finish(&mut self) -> Option<wgpu::CommandBuffer> {
        let enc = self.enc.take()?;
        self.close_active();
        for buf in self.used.drain(..) {
            let (tx, back) = (self.back_tx.clone(), buf.clone());
            enc.map_buffer_on_submit(&buf, wgpu::MapMode::Write, .., move |r| {
                if r.is_ok() {
                    let _ = tx.send(back);
                }
            });
        }
        Some(enc.finish())
    }

    fn close_active(&mut self) {
        if let Some(Active { buf, view, .. }) = self.active.take() {
            drop(view);
            buf.unmap();
            self.used.push(buf);
        }
    }

    /// Free chunk (back from the GPU) or new one, mapped.
    fn take_chunk(&mut self) -> wgpu::Buffer {
        while let Ok(b) = self.back_rx.try_recv() {
            if self.free.len() < MAX_FREE {
                self.free.push(b);
            }
        }
        self.free.pop().unwrap_or_else(|| {
            self.stats.transits += 1;
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("transit ring"),
                size: CHUNK,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            })
        })
    }
}

fn new_encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("transit") })
}

/// `N` uniforms of [`SLOT`] bytes of the frame (see the module header).
pub struct Slots<const N: usize> {
    merged: bool,
    skip: bool,
    /// One buffer of `N` slots (grouped) or one per slot.
    bufs: Vec<wgpu::Buffer>,
    cpu: [[u8; SLOT]; N],
    /// Last value written of each slot (skipping of unchanged values).
    sent: [Option<[u8; SLOT]>; N],
    dirty: [bool; N],
}

impl<const N: usize> Slots<N> {
    pub fn new(device: &wgpu::Device, labels: [&str; N], merged: bool, skip: bool) -> Self {
        let buffer = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                // Read back by the tests.
                usage: wgpu::BufferUsages::UNIFORM
                    | wgpu::BufferUsages::COPY_DST
                    | if cfg!(test) { wgpu::BufferUsages::COPY_SRC } else { wgpu::BufferUsages::empty() },
                mapped_at_creation: false,
            })
        };
        let bufs = if merged { vec![buffer(labels[0], N * SLOT)] } else { labels.map(|l| buffer(l, SLOT)).into() };
        Slots { merged, skip, bufs, cpu: [[0; SLOT]; N], sent: [None; N], dirty: [false; N] }
    }

    /// Binding of slot `i` (fixed offset in the grouped buffer).
    pub fn binding(&self, i: usize) -> wgpu::BindingResource<'_> {
        if self.merged {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &self.bufs[0],
                offset: (i * SLOT) as u64,
                size: wgpu::BufferSize::new(SLOT as u64),
            })
        } else {
            self.bufs[i].as_entire_binding()
        }
    }

    /// Value of slot `i` for this frame (written at the next [`Slots::flush`]).
    pub fn set(&mut self, i: usize, bytes: &[u8]) {
        self.cpu[i].copy_from_slice(bytes);
        self.dirty[i] = true;
    }

    /// Writes the slots set since the last call: after it, each GPU slot equals its
    /// CPU value (a skipped slot already equals its last value written, identical).
    pub fn flush(&mut self, up: &mut Uploader, queue: &wgpu::Queue) {
        let todo: [bool; N] =
            std::array::from_fn(|i| self.dirty[i] && !(self.skip && self.sent[i] == Some(self.cpu[i])));
        self.dirty = [false; N];
        if self.merged {
            let (Some(lo), Some(hi)) = (todo.iter().position(|&t| t), todo.iter().rposition(|&t| t)) else { return };
            up.write(queue, &self.bufs[0], (lo * SLOT) as u64, self.cpu[lo..=hi].as_flattened());
            for i in lo..=hi {
                self.sent[i] = Some(self.cpu[i]);
            }
        } else {
            for i in (0..N).filter(|&i| todo[i]) {
                up.write(queue, &self.bufs[i], 0, &self.cpu[i]);
                self.sent[i] = Some(self.cpu[i]);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! Proof on a real GPU: a deterministic script of 300 frames of writes (small,
    //! overlapping, larger than a chunk, buffers enlarged by a copy submitted in the middle
    //! like `alloc_in`, and a check copy in each frame's encoder that reads a written
    //! buffer) played by the old and the new path; final contents and log of the copies read back and
    //! compared with the CPU model. An omitted write makes the comparison fail (verified right here).
    //! Without a GPU the test skips itself; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.
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
        let adapter = rc_testdata::need(adapter, "GPU adapter")?;
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
    }

    const FRAMES: usize = 300;
    const PROBE: u64 = 64;
    const USAGE: wgpu::BufferUsages = wgpu::BufferUsages::COPY_DST.union(wgpu::BufferUsages::COPY_SRC);

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

    /// Plays the script on the GPU (`gpu` = Some) or on the CPU model; returns the final contents of the
    /// buffers, the log of the check copies and the counters.
    fn play(gpu: Option<(&wgpu::Device, &wgpu::Queue, bool, Option<u64>)>) -> (Vec<Vec<u8>>, Stats) {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut model: Vec<Vec<u8>> = [4096usize, 300 << 10, 3 << 20].iter().map(|&n| vec![0; n]).collect();
        let mut journal = vec![0u8; FRAMES * PROBE as usize];
        let mut dev = gpu.map(|(device, queue, ring, omit)| {
            let mut up = Uploader::new(device, ring);
            up.omit = omit;
            let bufs: Vec<wgpu::Buffer> = model
                .iter()
                .map(|m| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: m.len() as u64,
                        usage: USAGE,
                        mapped_at_creation: false,
                    })
                })
                .collect();
            let log = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: journal.len() as u64,
                usage: USAGE,
                mapped_at_creation: false,
            });
            (device, queue, up, bufs, log)
        });
        for frame in 0..FRAMES {
            let writes = 4 + rng.below(16);
            for w in 0..writes {
                let t = rng.below(3) as usize;
                // Larger than a chunk from time to time, otherwise small (often overlapping).
                let max = if t == 2 && rng.below(12) == 0 { 3 * CHUNK / 2 } else { 4096 };
                let size = model[t].len() as u64;
                let len = (4 + rng.below(max.min(size) - 4)) & !3;
                let off = rng.below(size - len) & !3;
                let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                model[t][off as usize..(off + len) as usize].copy_from_slice(&data);
                if let Some((_, queue, up, bufs, _)) = dev.as_mut() {
                    up.write(queue, &bufs[t], off, &data);
                }
                // Enlargement in the middle of the writes (copy submitted at once, like `alloc_in`).
                if w == 1 && frame % 50 == 7 {
                    let g = rng.below(2) as usize;
                    let grown = (model[g].len() + model[g].len() / 4).next_multiple_of(4);
                    if let Some((device, queue, up, bufs, _)) = dev.as_mut() {
                        let bigger = device.create_buffer(&wgpu::BufferDescriptor {
                            label: None,
                            size: grown as u64,
                            usage: USAGE,
                            mapped_at_creation: false,
                        });
                        let mut enc = device.create_command_encoder(&Default::default());
                        enc.copy_buffer_to_buffer(&bufs[g], 0, &bigger, 0, bufs[g].size());
                        up.submit(queue, enc.finish());
                        bufs[g] = bigger;
                    }
                    model[g].resize(grown, 0);
                }
            }
            // Check copy in the frame's encoder: reads what the writes placed.
            let at = rng.below(model[0].len() as u64 - PROBE) & !3;
            journal[frame * PROBE as usize..][..PROBE as usize]
                .copy_from_slice(&model[0][at as usize..(at + PROBE) as usize]);
            if let Some((device, queue, up, bufs, log)) = dev.as_mut() {
                let mut enc = device.create_command_encoder(&Default::default());
                enc.copy_buffer_to_buffer(&bufs[0], at, log, frame as u64 * PROBE, PROBE);
                up.submit(queue, enc.finish());
                if frame % 3 == 2 {
                    // Rhythm of a frame loop (a few frames in flight).
                    let _ = device.poll(wgpu::PollType::Poll);
                }
            }
        }
        match dev {
            Some((device, queue, mut up, bufs, log)) => {
                let mut out: Vec<Vec<u8>> = bufs.iter().map(|b| read(device, queue, b)).collect();
                out.push(read(device, queue, &log));
                (out, up.take_stats())
            }
            None => {
                model.push(journal);
                (model, Stats::default())
            }
        }
    }

    fn digest(v: &[Vec<u8>]) -> u64 {
        v.iter().flatten().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
    }

    #[test]
    fn ring_writes_match_queue_writes() {
        let Some((device, queue)) = gpu() else { return };
        let (expected, _) = play(None);
        let (old, old_stats) = play(Some((&device, &queue, false, None)));
        let (new, new_stats) = play(Some((&device, &queue, true, None)));
        eprintln!("old: {old_stats:?}; ring: {new_stats:?}; fingerprint {:016x}", digest(&expected));
        assert!(old == expected, "old path differs from the model");
        assert!(new == expected, "transit ring differs from the model");
        assert_eq!(old_stats.writes, new_stats.writes);
        assert_eq!(old_stats.transits, old_stats.writes, "old path: one transit per write");
        assert!(new_stats.transits * 4 < new_stats.writes, "ring: chunks recycled ({new_stats:?})");
        // Sabotage: an omitted write must show.
        let (bad, _) = play(Some((&device, &queue, true, Some(new_stats.writes - 1))));
        assert!(bad != expected, "omitted write not detected");
    }

    /// Without any `device.poll` (`RUSTCRAFT_OPT_NOPOLL`): chunks come back through submissions
    /// alone (wgpu delivers the mapping callbacks on every submit). Imposed rhythm: each
    /// frame waits, through empty submissions, for its chunk to come back; a callback delivered only by
    /// a poll would make the wait fail (and create a new chunk per frame).
    #[test]
    fn chunks_come_back_without_poll() {
        let Some((device, queue)) = gpu() else { return };
        let target = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 1 << 16,
            usage: USAGE,
            mapped_at_creation: false,
        });
        let mut up = Uploader::new(&device, true);
        let data = vec![5u8; 4096];
        for frame in 0..60 {
            up.write(&queue, &target, 0, &data);
            up.submit(&queue, device.create_command_encoder(&Default::default()).finish());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while up.back_rx.is_empty() {
                assert!(std::time::Instant::now() < deadline, "frame {frame}: chunk never returned without poll");
                queue.submit([]);
                std::thread::sleep(std::time::Duration::from_micros(200));
            }
        }
        let s = up.take_stats();
        assert_eq!((s.writes, s.transits), (60, 1), "a single chunk, recycled on every frame");
    }

    /// Plays 300 frames of slots (CPU model if `gpu` = None); returns the log (all the slots
    /// read back by a copy in the encoder of EVERY frame, hence what its draws see) and the
    /// number of writes.
    fn play_slots(gpu: Option<(&wgpu::Device, &wgpu::Queue, bool, bool, Option<u64>)>) -> (Vec<u8>, u64) {
        const N: usize = 4;
        const ROW: u64 = (N * SLOT) as u64;
        let mut rng = Rng(42);
        let mut model = [[0u8; SLOT]; N];
        let mut journal = Vec::with_capacity(FRAMES * N * SLOT);
        let mut dev = gpu.map(|(device, queue, merged, skip, omit)| {
            let mut up = Uploader::new(device, true);
            up.omit = omit;
            let slots = Slots::<N>::new(device, ["a", "b", "c", "d"], merged, skip);
            let log = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: FRAMES as u64 * ROW,
                usage: USAGE,
                mapped_at_creation: false,
            });
            (device, queue, up, slots, log)
        });
        // A few possible values per slot: the repetitions exercise the skipping.
        let pool: Vec<[u8; SLOT]> = (0..3).map(|_| std::array::from_fn(|_| rng.next() as u8)).collect();
        for frame in 0..FRAMES {
            for (i, slot) in model.iter_mut().enumerate() {
                if rng.below(3) != 0 {
                    let v = pool[rng.below(pool.len() as u64) as usize];
                    *slot = v;
                    if let Some((_, _, _, s, _)) = dev.as_mut() {
                        s.set(i, &v);
                    }
                }
            }
            journal.extend_from_slice(model.as_flattened());
            if let Some((device, queue, up, s, log)) = dev.as_mut() {
                s.flush(up, queue);
                let mut enc = device.create_command_encoder(&Default::default());
                let base = frame as u64 * ROW;
                if s.merged {
                    enc.copy_buffer_to_buffer(&s.bufs[0], 0, log, base, ROW);
                } else {
                    for (i, b) in s.bufs.iter().enumerate() {
                        enc.copy_buffer_to_buffer(b, 0, log, base + (i * SLOT) as u64, SLOT as u64);
                    }
                }
                up.submit(queue, enc.finish());
            }
        }
        match dev {
            Some((device, queue, mut up, _, log)) => (read(device, queue, &log), up.take_stats().writes),
            None => (journal, 0),
        }
    }

    #[test]
    fn slots_match_separate_buffers() {
        let Some((device, queue)) = gpu() else { return };
        let (expected, _) = play_slots(None);
        let mut writes = Vec::new();
        for (merged, skip) in [(false, false), (false, true), (true, false), (true, true)] {
            let (got, w) = play_slots(Some((&device, &queue, merged, skip, None)));
            // Frame-by-frame comparison: the first wrong frame is named.
            let row = 4 * SLOT;
            let bad = got.chunks(row).zip(expected.chunks(row)).position(|(g, e)| g != e);
            assert!(got.len() == expected.len() && bad.is_none(), "merged {merged}, skip {skip}: frame {bad:?} wrong");
            writes.push(w);
        }
        eprintln!("writes (separate, separate+skip, merged, merged+skip): {writes:?}");
        assert!(writes[3] < writes[1] && writes[1] < writes[0] && writes[2] < writes[0]);
        // Transient sabotage: a write omitted in the middle (slot wrong for a few frames, then
        // rewritten) must show in the log, even if the final state is right.
        let (bad, _) = play_slots(Some((&device, &queue, true, true, Some(writes[3] / 2))));
        let first = bad.chunks(4 * SLOT).zip(expected.chunks(4 * SLOT)).position(|(g, e)| g != e);
        let last_ok = bad[bad.len() - 4 * SLOT..] == expected[expected.len() - 4 * SLOT..];
        eprintln!("sabotage: first wrong frame {first:?}, last correct frame {last_ok}");
        assert!(first.is_some(), "omitted write not detected");
    }

    /// Micro-bench (main thread, single GPU queue: no multi-core version): 15 small
    /// writes per frame + submission, old path against ring.
    /// `cargo test --release -p rc-app --bin rustcraft upload_bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn upload_bench() {
        let Some((device, queue)) = gpu() else { return };
        let target = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 1 << 20,
            usage: USAGE,
            mapped_at_creation: false,
        });
        // Typical frame (cameras, 64 KiB fade, orders, indirect commands) and small writes alone.
        let image: &[u64] = &[256, 256, 256, 256, 64 << 10, 4096, 2048, 512, 512, 512, 512, 512, 8192, 8, 1024];
        let small: &[u64] = &[256; 15];
        let data = vec![7u8; 64 << 10];
        for pass in 0..3 {
            for (set, sizes) in [("frame", image), ("small", small)] {
                for ring in [false, true] {
                    let mut up = Uploader::new(&device, ring);
                    let frames = 3000;
                    let t = std::time::Instant::now();
                    let (mut write_ns, mut submit_ns) = (0u128, 0u128);
                    for f in 0..frames {
                        let tw = std::time::Instant::now();
                        let mut at = 0;
                        for &s in sizes {
                            up.write(&queue, &target, at, &data[..s as usize]);
                            at += s.next_multiple_of(256);
                        }
                        write_ns += tw.elapsed().as_nanos();
                        let enc = device.create_command_encoder(&Default::default());
                        let ts = std::time::Instant::now();
                        let idx = up.submit(&queue, enc.finish());
                        submit_ns += ts.elapsed().as_nanos();
                        if f % 3 == 2 {
                            let _ = device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None });
                        }
                    }
                    let us = |ns: u128| ns as f64 / 1e3 / f64::from(frames);
                    eprintln!(
                        "pass {pass} {set} {}: {:.1} µs/frame (writes {:.1} µs, submit {:.1} µs), {:?}",
                        if ring { "ring" } else { "old" },
                        t.elapsed().as_secs_f64() * 1e6 / f64::from(frames),
                        us(write_ns),
                        us(submit_ns),
                        up.take_stats()
                    );
                }
            }
        }
    }
}
