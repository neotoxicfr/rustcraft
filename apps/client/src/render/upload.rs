//! Uploads to the GPU buffers and textures, common to the whole renderer.
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
//!   wait for the next submission); a copy that must stay in order with the writes (a growing
//!   buffer moving its contents) goes through [`Uploader::copy_buffer`], never through a
//!   submission of its own;
//! - a buffer or texture written here is never also written by the queue (`write_buffer`,
//!   `write_texture`): within one submit, the queue writes would go before the transit and the
//!   order of the calls would be lost. Debug builds and tests check it for the queue writes made
//!   here (the `=0` paths of the switches below);
//! - a texture written here that grows by a GPU copy has that copy recorded here too: a direct
//!   submission would run before the pending writes and lose them (the glyph pages and the entity
//!   textures grow that way, with their writes still on the queue);
//! - a write larger than a chunk has its own disposable buffer (same cost as before);
//!   beyond `MAX_FREE` free chunks, the chunks returned are destroyed (bounded memory).
//!
//! Switches, read once when the uploader is created (default 1, `=0` = the path before):
//! - `RUSTCRAFT_OPT_FRAME_WRITES_RING`: the per-frame layers' buffers ([`per_frame::FrameBuffer`])
//!   written through the ring; `=0`: through the queue, for the A/B of each migrated layer in one exe;
//! - `RUSTCRAFT_OPT_RING_TEXTURE_WRITES`: [`Uploader::write_texture`] through the ring, rows
//!   padded to 256 bytes in the chunk; `=0`: the queue's texture write;
//! - `RUSTCRAFT_OPT_GROWTH_IN_TRANSIT`: [`Uploader::copy_buffer`] recorded in the transit in call
//!   order; `=0`: submitted at once after the pending transit (one more command buffer);
//! - `RUSTCRAFT_OPT_SINGLE_COMMAND_BUFFER`: [`Uploader::frame_encoder`] hands out the transit
//!   encoder, the pending writes at its head (one command buffer per submission instead of two).
//!   A write made after that call goes into a new transit submitted before the frame, so it must
//!   not touch a resource written before the call (checked in debug builds and tests);
//! - `RUSTCRAFT_OPT_ADAPTIVE_UPLOAD_CHUNK`: chunks of max(1 MiB, power of two above the largest
//!   write of the last 60 submissions), 8 MiB at most: a large write made on every frame (the
//!   matrices of a crowd) reuses its chunk instead of a disposable buffer per frame;
//! - `RUSTCRAFT_OPT_MERGE_UPLOAD_COPIES`: a write that continues the previous one (same target, same
//!   chunk, contiguous on both sides) extends its GPU copy instead of recording another.
//!
//! **Frame uniforms** ([`Slots`], `RUSTCRAFT_OPT_FRAMEUNIF`, `RUSTCRAFT_OPT_UNIFSKIP`):
//! N values of 256 bytes (one camera per slot) in a single buffer with fixed offsets, one binding
//! per slot, a single write of the contiguous range of the changed slots; a value identical to
//! the last one written is not rewritten. Old path: one buffer per value, rewritten on every
//! frame. The slot buffers are never recreated (a surface resume keeps them); a
//! subsystem that recreated its own must forget the last values written (`sent`).

pub(crate) mod per_frame;

use crossbeam_channel::{Receiver, Sender};

/// Size of a ring chunk (the smallest one with adaptive chunks); a larger write has its
/// disposable buffer.
const CHUNK: u64 = 1 << 20;
/// Largest adaptive chunk: a larger write keeps its disposable buffer, and the chunks kept stay
/// under `MAX_FREE` × 8 MiB.
const MAX_CHUNK: u64 = 8 << 20;
/// Submissions whose largest write sets the size of the adaptive chunks.
const CHUNK_WINDOW: usize = 60;
/// Free chunks kept at most (beyond, given back to the driver).
const MAX_FREE: usize = 8;
/// Size of a uniform slot (maximum alignment of the uniform binding offsets).
pub const SLOT: usize = 256;

/// Transit ring active (`RUSTCRAFT_OPT_UPLOADRING`, default 1), read once.
pub fn ring_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_UPLOADRING")
}

/// Frame uniforms grouped (`RUSTCRAFT_OPT_FRAMEUNIF`, default 1), read once.
pub fn merged_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_FRAMEUNIF")
}

/// Writes of unchanged uniforms skipped (`RUSTCRAFT_OPT_UNIFSKIP`, default 1), read once.
pub fn skip_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_UNIFSKIP")
}

/// Behaviour of the ring itself (module header), read once; the tests set it directly.
#[derive(Debug, Clone, Copy)]
struct Options {
    growth_in_transit: bool,
    adaptive_chunk: bool,
    merge_copies: bool,
}

impl Options {
    fn from_switches() -> Self {
        Options {
            growth_in_transit: rc_data::opt_switch!("RUSTCRAFT_OPT_GROWTH_IN_TRANSIT"),
            adaptive_chunk: rc_data::opt_switch!("RUSTCRAFT_OPT_ADAPTIVE_UPLOAD_CHUNK"),
            merge_copies: rc_data::opt_switch!("RUSTCRAFT_OPT_MERGE_UPLOAD_COPIES"),
        }
    }
}

/// Counters since the last sample: writes, transit buffers created, bytes written, GPU copies
/// recorded (after merges) and command buffers submitted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub writes: u64,
    pub transits: u64,
    pub bytes: u64,
    pub copies: u64,
    pub command_buffers: u64,
}

/// Chunk being filled: its mapped view (kept while it is active) and the cursor.
struct Active {
    buf: wgpu::Buffer,
    view: wgpu::BufferViewMut,
    at: u64,
}

/// GPU copy of a write, from the transit into its target.
struct BufferCopy {
    src: wgpu::Buffer,
    src_offset: u64,
    dst: wgpu::Buffer,
    dst_offset: u64,
    len: u64,
}

impl BufferCopy {
    /// True if `next` continues this copy on both sides (same chunk and same target, contiguous).
    fn continued_by(&self, next: &BufferCopy) -> bool {
        self.src == next.src
            && self.dst == next.dst
            && self.src_offset + self.len == next.src_offset
            && self.dst_offset + self.len == next.dst_offset
    }

    fn record(&self, enc: &mut wgpu::CommandEncoder) {
        enc.copy_buffer_to_buffer(&self.src, self.src_offset, &self.dst, self.dst_offset, self.len);
    }
}

pub struct Uploader {
    device: wgpu::Device,
    ring: bool,
    opts: Options,
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the per-frame uploads once the layers migrate"))]
    layer_opts: per_frame::LayerOptions,
    enc: Option<wgpu::CommandEncoder>,
    active: Option<Active>,
    /// Copy of the last write, not recorded yet (`merge_copies`): the next write may extend it.
    pending: Option<BufferCopy>,
    /// Chunks written since the last submission (unmapped), returned after being read.
    used: Vec<wgpu::Buffer>,
    /// Chunks back from the GPU, mapped and empty.
    free: Vec<wgpu::Buffer>,
    back_tx: Sender<wgpu::Buffer>,
    back_rx: Receiver<wgpu::Buffer>,
    chunk_size: ChunkSizer,
    rules: rules::RuleCheck,
    stats: Stats,
    #[cfg(test)]
    sabotage: Sabotage,
}

/// Faults injected by the tests, each of which must make an equivalence proof fail.
#[cfg(test)]
#[derive(Default)]
struct Sabotage {
    /// Number of the omitted write.
    omit: Option<u64>,
    /// Texture rows staged without their padding (the declared stride stays padded).
    unpadded_rows: bool,
    /// Growth copies recorded after all the writes of the transit instead of in call order.
    growth_copies_last: bool,
    delayed_growth_copies: Vec<BufferCopy>,
}

impl Uploader {
    pub fn new(device: &wgpu::Device, ring: bool) -> Self {
        Self::with_options(device, ring, Options::from_switches(), per_frame::LayerOptions::from_switches())
    }

    fn with_options(device: &wgpu::Device, ring: bool, opts: Options, layer_opts: per_frame::LayerOptions) -> Self {
        let (back_tx, back_rx) = crossbeam_channel::unbounded();
        Uploader {
            device: device.clone(),
            ring,
            opts,
            layer_opts,
            enc: None,
            active: None,
            pending: None,
            used: Vec::new(),
            free: Vec::new(),
            back_tx,
            back_rx,
            chunk_size: ChunkSizer::new(opts.adaptive_chunk),
            rules: rules::RuleCheck::new(),
            stats: Stats::default(),
            #[cfg(test)]
            sabotage: Sabotage::default(),
        }
    }

    /// Writes `data` into `target` at `offset` (multiples of 4), effective at the next submission.
    pub fn write(&mut self, queue: &wgpu::Queue, target: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.write_routed(queue, target, offset, data, self.ring);
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
        self.write_with_routed(queue, target, offset, len, fill, self.ring);
    }

    /// Copies `len` bytes of `src` into `dst` in order with the writes (a growing buffer moving its
    /// contents into the larger one): recorded in the transit (`RUSTCRAFT_OPT_GROWTH_IN_TRANSIT`),
    /// or submitted at once after the pending transit (`=0`, and always without the ring, where
    /// the queue writes that follow must land after the copy).
    pub fn copy_buffer(
        &mut self,
        queue: &wgpu::Queue,
        src: &wgpu::Buffer,
        src_offset: u64,
        dst: &wgpu::Buffer,
        dst_offset: u64,
        len: u64,
    ) {
        if !(self.ring && self.opts.growth_in_transit) {
            let mut enc = new_encoder(&self.device, "buffer growth");
            enc.copy_buffer_to_buffer(src, src_offset, dst, dst_offset, len);
            self.submit(queue, enc.finish());
            return;
        }
        self.rules.ring_reads(src);
        self.rules.ring_writes(dst);
        let copy = BufferCopy { src: src.clone(), src_offset, dst: dst.clone(), dst_offset, len };
        #[cfg(test)]
        if self.sabotage.growth_copies_last {
            self.sabotage.delayed_growth_copies.push(copy);
            return;
        }
        self.record_pending();
        self.record(&copy);
    }

    /// Submits `cmd` preceded by the transit of the pending writes.
    pub fn submit(&mut self, queue: &wgpu::Queue, cmd: wgpu::CommandBuffer) -> wgpu::SubmissionIndex {
        let index = match self.finish() {
            Some(transit) => {
                self.stats.command_buffers += 2;
                queue.submit([transit, cmd])
            }
            None => {
                self.stats.command_buffers += 1;
                queue.submit([cmd])
            }
        };
        self.chunk_size.end_submission();
        self.rules.submitted();
        index
    }

    /// Counters since the last call.
    pub fn take_stats(&mut self) -> Stats {
        std::mem::take(&mut self.stats)
    }

    /// Counts a write of `len` bytes; false if a test sabotage omits it.
    fn counted(&mut self, len: u64) -> bool {
        self.stats.writes += 1;
        #[cfg(test)]
        if self.sabotage.omit == Some(self.stats.writes - 1) {
            return false;
        }
        self.stats.bytes += len;
        true
    }

    fn write_routed(&mut self, queue: &wgpu::Queue, target: &wgpu::Buffer, offset: u64, data: &[u8], ring: bool) {
        if data.is_empty() || !self.counted(data.len() as u64) {
            return;
        }
        if ring {
            self.ring_write(target, offset, data.len() as u64, |mut view| view.copy_from_slice(data));
        } else {
            self.rules.queue_writes(target);
            self.stats.transits += 1;
            queue.write_buffer(target, offset, data);
        }
    }

    fn write_with_routed(
        &mut self,
        queue: &wgpu::Queue,
        target: &wgpu::Buffer,
        offset: u64,
        len: u64,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
        ring: bool,
    ) {
        let Some(size) = wgpu::BufferSize::new(len) else { return };
        if !self.counted(len) {
            return;
        }
        if ring {
            self.ring_write(target, offset, len, fill);
        } else {
            self.rules.queue_writes(target);
            self.stats.transits += 1;
            let mut view = queue.write_buffer_with(target, offset, size).expect("valid queue write");
            fill(view.slice(..));
        }
    }

    /// Write through the transit: staged in a chunk, then a GPU copy into `target`.
    fn ring_write(
        &mut self,
        target: &wgpu::Buffer,
        offset: u64,
        len: u64,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
    ) {
        debug_assert!(
            len.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) && offset.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
        );
        self.rules.ring_writes(target);
        let (src, src_offset) = self.stage(len, wgpu::COPY_BUFFER_ALIGNMENT, fill);
        let copy = BufferCopy { src, src_offset, dst: target.clone(), dst_offset: offset, len };
        if !self.opts.merge_copies {
            self.record(&copy);
            return;
        }
        // Kept unrecorded: a run of contiguous writes (fade slots, neighbouring columns) then costs
        // one GPU copy instead of one per write.
        match &mut self.pending {
            Some(last) if last.continued_by(&copy) => last.len += copy.len,
            _ => {
                self.record_pending();
                self.pending = Some(copy);
            }
        }
    }

    /// Room for `len` bytes in the transit at an offset aligned to `align`, filled by `fill`: the
    /// source buffer and offset of its GPU copy. A write larger than a chunk gets a disposable buffer.
    fn stage(&mut self, len: u64, align: u64, fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>)) -> (wgpu::Buffer, u64) {
        self.chunk_size.note_write(len);
        if len > self.chunk_size.current() {
            return (self.disposable(len, fill), 0);
        }
        let at = self.room_in_active_chunk(len, align);
        let active = self.active.as_mut().expect("active chunk");
        fill(active.view.slice(at as usize..(at + len) as usize));
        active.at = at + len;
        (active.buf.clone(), at)
    }

    /// Offset of `len` bytes in the active chunk; a new chunk is taken when it has no room left.
    fn room_in_active_chunk(&mut self, len: u64, align: u64) -> u64 {
        if let Some(active) = &self.active {
            let at = active.at.next_multiple_of(align);
            if at + len <= active.buf.size() {
                return at;
            }
        }
        self.close_active();
        let buf = self.take_chunk();
        let view = buf.slice(..).get_mapped_range_mut().expect("transit chunk mapped");
        self.active = Some(Active { buf, view, at: 0 });
        0
    }

    /// Transit of its own for a write larger than a chunk, destroyed once its copy has run.
    fn disposable(&mut self, len: u64, fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>)) -> wgpu::Buffer {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transit (large upload)"),
            size: len,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        fill(buf.slice(..).get_mapped_range_mut().expect("transit mapped at creation").slice(..));
        buf.unmap();
        self.stats.transits += 1;
        buf
    }

    /// Records the copy kept for a merge: every later command lands after it (call order).
    fn record_pending(&mut self) {
        if let Some(copy) = self.pending.take() {
            self.record(&copy);
        }
    }

    fn record(&mut self, copy: &BufferCopy) {
        self.stats.copies += 1;
        copy.record(self.encoder());
    }

    fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        let device = &self.device;
        self.enc.get_or_insert_with(|| new_encoder(device, "transit"))
    }

    /// Closes the transit: chunks unmapped, returned to the ring when the GPU has read them.
    fn finish(&mut self) -> Option<wgpu::CommandBuffer> {
        self.record_pending();
        #[cfg(test)]
        for copy in std::mem::take(&mut self.sabotage.delayed_growth_copies) {
            self.record(&copy);
        }
        let enc = self.enc.take()?;
        self.close_active();
        self.return_chunks_after(&enc);
        Some(enc.finish())
    }

    /// The chunks written since the last submission go back to the ring once `enc` has run.
    fn return_chunks_after(&mut self, enc: &wgpu::CommandEncoder) {
        for buf in self.used.drain(..) {
            let (tx, back) = (self.back_tx.clone(), buf.clone());
            enc.map_buffer_on_submit(&buf, wgpu::MapMode::Write, .., move |r| {
                if r.is_ok() {
                    let _ = tx.send(back);
                }
            });
        }
    }

    fn close_active(&mut self) {
        if let Some(Active { buf, view, .. }) = self.active.take() {
            drop(view);
            buf.unmap();
            self.used.push(buf);
        }
    }

    /// Free chunk (back from the GPU) or new one, mapped, of the current chunk size.
    fn take_chunk(&mut self) -> wgpu::Buffer {
        let size = self.chunk_size.current();
        while let Ok(b) = self.back_rx.try_recv() {
            if self.free.len() < MAX_FREE {
                self.free.push(b);
            }
        }
        // Chunks of an older size (adaptive chunks) are given back to the driver.
        self.free.retain(|b| b.size() == size);
        self.free.pop().unwrap_or_else(|| {
            self.stats.transits += 1;
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("transit ring"),
                size,
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            })
        })
    }
}

fn new_encoder(device: &wgpu::Device, label: &str) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) })
}

/// Size of the next ring chunks. Adaptive: max(1 MiB, power of two above the largest write of the
/// last `CHUNK_WINDOW` submissions), `MAX_CHUNK` at most; otherwise always 1 MiB.
struct ChunkSizer {
    adaptive: bool,
    current: u64,
    /// Largest write (up to `MAX_CHUNK`) of each of the last submissions, in a circle.
    largest: [u64; CHUNK_WINDOW],
    slot: usize,
}

impl ChunkSizer {
    fn new(adaptive: bool) -> Self {
        ChunkSizer { adaptive, current: CHUNK, largest: [0; CHUNK_WINDOW], slot: 0 }
    }

    fn current(&self) -> u64 {
        self.current
    }

    /// A write of `len` bytes: with adaptive chunks, one that stays under `MAX_CHUNK` raises the
    /// size at once (its chunk is created now and recycled afterwards).
    fn note_write(&mut self, len: u64) {
        if !self.adaptive || len > MAX_CHUNK {
            return;
        }
        self.largest[self.slot] = self.largest[self.slot].max(len);
        self.current = self.current.max(len.next_power_of_two());
    }

    /// End of a submission: the window moves on and the size follows its largest write (back to
    /// 1 MiB once the large writes have stopped for a whole window).
    fn end_submission(&mut self) {
        if !self.adaptive {
            return;
        }
        self.slot = (self.slot + 1) % CHUNK_WINDOW;
        self.largest[self.slot] = 0;
        let largest = self.largest.iter().copied().max().unwrap_or(0);
        self.current = largest.next_power_of_two().max(CHUNK);
    }
}

/// Check of the rules of use (module header), in debug builds and tests only: a resource written
/// by the ring is never written by the queue afterwards, and after [`Uploader::frame_encoder`] a
/// late write or growth copy never touches a resource already written at the head of the frame
/// encoder. Per resource, not per range: conservative.
#[cfg(any(test, debug_assertions))]
mod rules {
    use std::any::TypeId;
    use std::hash::Hash;

    use hashbrown::HashSet;

    /// Identity of a buffer or texture that does not keep it alive (wgpu ids are never reused;
    /// the type keeps a buffer and a texture with the same id apart).
    fn key<T: Hash + 'static>(resource: &T) -> u64 {
        crate::cache_key::fast_cache_key(&(TypeId::of::<T>(), resource))
    }

    #[derive(Default)]
    pub(super) struct RuleCheck {
        /// Every resource ever written by the ring.
        ring_written: HashSet<u64>,
        /// Resources written since the last submission.
        this_submission: Vec<u64>,
        /// Resources written at the head of the frame encoder handed out, until the submission.
        at_head: Vec<u64>,
    }

    impl RuleCheck {
        pub(super) fn new() -> Self {
            Self::default()
        }

        pub(super) fn ring_writes<T: Hash + 'static>(&mut self, resource: &T) {
            let k = key(resource);
            assert!(
                !self.at_head.contains(&k),
                "late write to a resource already written at the head of the frame encoder: it would land first"
            );
            self.ring_written.insert(k);
            self.this_submission.push(k);
        }

        pub(super) fn ring_reads<T: Hash + 'static>(&mut self, resource: &T) {
            assert!(
                !self.at_head.contains(&key(resource)),
                "late copy from a resource written at the head of the frame encoder: it would read the old contents"
            );
        }

        pub(super) fn queue_writes<T: Hash + 'static>(&self, resource: &T) {
            assert!(
                !self.ring_written.contains(&key(resource)),
                "queue write to a resource written by the ring: it would land before the ring's pending writes"
            );
        }

        #[cfg_attr(not(test), expect(dead_code, reason = "first caller: the frame encoder once wired"))]
        pub(super) fn frame_encoder_taken(&mut self) {
            self.at_head.append(&mut self.this_submission);
        }

        pub(super) fn submitted(&mut self) {
            self.this_submission.clear();
            self.at_head.clear();
        }
    }
}

/// Release builds: the rules are not checked (nothing computed).
#[cfg(not(any(test, debug_assertions)))]
mod rules {
    pub(super) struct RuleCheck;

    impl RuleCheck {
        pub(super) fn new() -> Self {
            RuleCheck
        }

        #[inline(always)]
        pub(super) fn ring_writes<T>(&mut self, _: &T) {}
        #[inline(always)]
        pub(super) fn ring_reads<T>(&mut self, _: &T) {}
        #[inline(always)]
        pub(super) fn queue_writes<T>(&self, _: &T) {}
        #[expect(dead_code, reason = "first caller: the frame encoder once wired")]
        #[inline(always)]
        pub(super) fn frame_encoder_taken(&mut self) {}
        #[inline(always)]
        pub(super) fn submitted(&mut self) {}
    }
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
#[path = "upload_test.rs"]
pub(crate) mod tests;
