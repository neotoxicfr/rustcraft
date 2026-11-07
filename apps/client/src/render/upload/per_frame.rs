//! Uploads made for the per-frame layers (entities, particles, weather, outlines, texts…): the
//! buffer each layer rewrites on every frame ([`FrameBuffer`]), the texture writes through the ring
//! ([`Uploader::write_texture`]) and the frame's encoder with the writes at its head
//! ([`Uploader::frame_encoder`]). Their switches are described in the header of the parent module.
//!
//! The layers move their writes here one by one (each with its own A/B), the frame encoder once
//! every write of the frame precedes it: until then nothing outside the tests calls this module.
#![cfg_attr(not(test), expect(dead_code, reason = "first callers: the migrations of the per-frame layers"))]

use super::{Uploader, new_encoder};

/// Alignment of a texture copy's source offset in a chunk: Direct3D 12's placement alignment, the
/// strictest backend (the queue's own texture write starts its transit at 0).
const TEXTURE_COPY_OFFSET_ALIGNMENT: u64 = 512;

/// Switches of the uploads of this module, read once when the uploader is created.
#[derive(Debug, Clone, Copy)]
pub(super) struct LayerOptions {
    /// [`FrameBuffer`] writes through the ring (`RUSTCRAFT_OPT_FRAME_WRITES_RING`).
    pub(super) frame_writes: bool,
    /// [`Uploader::write_texture`] through the ring (`RUSTCRAFT_OPT_RING_TEXTURE_WRITES`).
    pub(super) texture_writes: bool,
    /// [`Uploader::frame_encoder`] hands out the transit encoder (`RUSTCRAFT_OPT_SINGLE_COMMAND_BUFFER`).
    pub(super) single_command_buffer: bool,
}

impl LayerOptions {
    pub(super) fn from_switches() -> Self {
        LayerOptions {
            frame_writes: rc_data::opt_switch!("RUSTCRAFT_OPT_FRAME_WRITES_RING"),
            texture_writes: rc_data::opt_switch!("RUSTCRAFT_OPT_RING_TEXTURE_WRITES"),
            single_command_buffer: rc_data::opt_switch!("RUSTCRAFT_OPT_SINGLE_COMMAND_BUFFER"),
        }
    }
}

impl Uploader {
    /// Write of a per-frame layer's buffer: through the ring, or through the queue with
    /// `RUSTCRAFT_OPT_FRAME_WRITES_RING=0`.
    pub(super) fn write_layer(&mut self, queue: &wgpu::Queue, target: &wgpu::Buffer, data: &[u8]) {
        self.write_routed(queue, target, 0, data, self.ring && self.layer_opts.frame_writes);
    }

    pub(super) fn write_layer_with(
        &mut self,
        queue: &wgpu::Queue,
        target: &wgpu::Buffer,
        len: u64,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
    ) {
        self.write_with_routed(queue, target, 0, len, fill, self.ring && self.layer_opts.frame_writes);
    }

    /// Writes `data`, laid out as `layout` says, into the `size` region of `texture`, effective at
    /// the next submission: through the ring with the rows padded to 256 bytes in the chunk (what
    /// a buffer-to-texture copy requires), or the queue's texture write
    /// (`RUSTCRAFT_OPT_RING_TEXTURE_WRITES=0`, and without the ring).
    pub fn write_texture(
        &mut self,
        queue: &wgpu::Queue,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        let rows = TextureRows::new(texture.texture.format(), texture.aspect, layout, size);
        if rows.texel_bytes() == 0 || !self.counted(rows.texel_bytes()) {
            return;
        }
        if !(self.ring && self.layer_opts.texture_writes) {
            self.rules.queue_writes(texture.texture);
            self.stats.transits += 1;
            queue.write_texture(texture, data, layout, size);
            return;
        }
        self.rules.ring_writes(texture.texture);
        #[cfg(test)]
        let staged_row = if self.sabotage.unpadded_rows { rows.row_bytes } else { rows.padded_row };
        #[cfg(not(test))]
        let staged_row = rows.padded_row;
        let (src, offset) =
            self.stage(rows.staged_len(), TEXTURE_COPY_OFFSET_ALIGNMENT, |view| rows.copy_into(data, view, staged_row));
        self.record_pending();
        self.stats.copies += 1;
        let source = wgpu::TexelCopyBufferInfo { buffer: &src, layout: rows.staged_layout(offset) };
        self.encoder().copy_buffer_to_texture(source, texture, size);
    }

    /// Encoder for the frame's commands. With `RUSTCRAFT_OPT_SINGLE_COMMAND_BUFFER` it is the
    /// transit encoder, the pending writes already at its head: one command buffer fewer per
    /// submission. A write made after this call goes into a new transit submitted before the frame,
    /// so it must not touch a resource written before the call (it would land before that earlier
    /// write): checked in debug builds and tests, per resource.
    pub fn frame_encoder(&mut self) -> wgpu::CommandEncoder {
        if !(self.ring && self.layer_opts.single_command_buffer) {
            return new_encoder(&self.device, "frame");
        }
        self.record_pending();
        self.close_active();
        let enc = self.enc.take().unwrap_or_else(|| new_encoder(&self.device, "frame"));
        self.return_chunks_after(&enc);
        self.rules.frame_encoder_taken();
        enc
    }
}

/// Rows of a texture write: read with the caller's layout, staged with each row at a padded
/// stride (`COPY_BYTES_PER_ROW_ALIGNMENT`), the stride a buffer-to-texture copy requires.
struct TextureRows {
    /// Bytes of one row of texel blocks, and its padded stride in the transit.
    row_bytes: u64,
    padded_row: u64,
    /// Rows of blocks per image, and images (depth or array layers).
    rows: u64,
    images: u64,
    /// Where the rows are in the caller's data.
    src_offset: u64,
    src_row_stride: u64,
    src_rows_per_image: u64,
}

impl TextureRows {
    fn new(
        format: wgpu::TextureFormat,
        aspect: wgpu::TextureAspect,
        layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) -> Self {
        let (block_width, block_height) = format.block_dimensions();
        let block_bytes = format.block_copy_size(Some(aspect)).expect("texture aspect that can be copied");
        let row_bytes = u64::from(size.width.div_ceil(block_width) * block_bytes);
        let rows = u64::from(size.height.div_ceil(block_height));
        TextureRows {
            row_bytes,
            padded_row: row_bytes.next_multiple_of(u64::from(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)),
            rows,
            images: u64::from(size.depth_or_array_layers),
            src_offset: layout.offset,
            src_row_stride: layout.bytes_per_row.map_or(row_bytes, u64::from),
            src_rows_per_image: layout.rows_per_image.map_or(rows, u64::from),
        }
    }

    /// Bytes of texels written (the padding left out).
    fn texel_bytes(&self) -> u64 {
        self.row_bytes * self.rows * self.images
    }

    /// Bytes staged in the transit: every row at its padded place, the last one unpadded.
    fn staged_len(&self) -> u64 {
        (self.rows * self.images - 1) * self.padded_row + self.row_bytes
    }

    /// Layout of the staged rows for the copy, starting at `offset` in the chunk.
    fn staged_layout(&self, offset: u64) -> wgpu::TexelCopyBufferLayout {
        let padded_row = u32::try_from(self.padded_row).expect("row stride within u32");
        let rows = u32::try_from(self.rows).expect("rows within u32");
        wgpu::TexelCopyBufferLayout { offset, bytes_per_row: Some(padded_row), rows_per_image: Some(rows) }
    }

    /// Copies the rows of `data` into `view`, each `staged_row` bytes after the previous one.
    fn copy_into(&self, data: &[u8], mut view: wgpu::WriteOnly<'_, [u8]>, staged_row: u64) {
        let (start, row) = (self.src_offset as usize, self.row_bytes as usize);
        // Rows already at the staged stride and images back to back: one copy of the whole block.
        if self.src_row_stride == staged_row && (self.images == 1 || self.src_rows_per_image == self.rows) {
            let len = ((self.rows * self.images - 1) * staged_row) as usize + row;
            view.slice(..len).copy_from_slice(&data[start..start + len]);
            return;
        }
        for image in 0..self.images {
            for r in 0..self.rows {
                let from = start + ((image * self.src_rows_per_image + r) * self.src_row_stride) as usize;
                let to = ((image * self.rows + r) * staged_row) as usize;
                view.slice(to..to + row).copy_from_slice(&data[from..from + row]);
            }
        }
    }
}

/// GPU buffer that a layer rewrites on every frame (vertices, instances or a uniform), recreated a
/// power of two larger when its data no longer fits: the growth each layer wrote by hand. Its
/// writes take the transit ring (`RUSTCRAFT_OPT_FRAME_WRITES_RING`; `=0`: the queue write, the path
/// before, for the A/B of each layer in one exe).
pub struct FrameBuffer {
    label: &'static str,
    usage: wgpu::BufferUsages,
    min_capacity: u64,
    buffer: Option<wgpu::Buffer>,
}

impl FrameBuffer {
    /// Created at its first write with `usage` plus the copy destination, `min_capacity` bytes at least.
    pub const fn new(label: &'static str, usage: wgpu::BufferUsages, min_capacity: u64) -> Self {
        FrameBuffer { label, usage, min_capacity, buffer: None }
    }

    pub fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }

    /// Room for `size` bytes (more than 0): true if the buffer was (re)created, empty; the bindings
    /// that read it must then be made again.
    pub fn reserve(&mut self, device: &wgpu::Device, size: u64) -> bool {
        if self.buffer.as_ref().is_some_and(|b| b.size() >= size) {
            return false;
        }
        self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size: size.next_power_of_two().max(self.min_capacity),
            usage: self.usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        true
    }

    /// Writes `data` at the start of the buffer, grown first if needed; true if it was (re)created.
    /// Nothing is written for empty data.
    pub fn write(&mut self, up: &mut Uploader, queue: &wgpu::Queue, data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }
        let fresh = self.reserve(&up.device, data.len() as u64);
        up.write_layer(queue, self.buffer.as_ref().expect("reserved above"), data);
        fresh
    }

    /// Like [`FrameBuffer::write`], the `len` bytes being filled by `fill` directly in the transit.
    pub fn write_with(
        &mut self,
        up: &mut Uploader,
        queue: &wgpu::Queue,
        len: u64,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
    ) -> bool {
        if len == 0 {
            return false;
        }
        let fresh = self.reserve(&up.device, len);
        up.write_layer_with(queue, self.buffer.as_ref().expect("reserved above"), len, fill);
        fresh
    }
}
