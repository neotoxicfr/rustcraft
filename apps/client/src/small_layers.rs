//! What the small per-frame layers share (targeted block outline, debug overlays, name tags, sign
//! and display texts, panorama): the form of their vertex shaders and the buffers sent only when
//! their bytes change.
//!
//! Switches, read once (default 1, `=0` = the path before):
//! - `RUSTCRAFT_OPT_PACKED_SHADER_CONSTANTS` (shared with the clouds): the corner tables of the
//!   vertex shaders are the bits of a constant instead of an array literal indexed by the vertex
//!   index, which a driver may copy into local memory for every vertex;
//! - `RUSTCRAFT_OPT_SKIP_UNCHANGED_UNIFORMS`: a small uniform (line style, screen size) is written
//!   only when its bytes change, instead of on every frame.

use crate::render::upload::Uploader;
use crate::render::upload::per_frame::FrameBuffer;

/// Extra usage of the layers' buffers: read back by the tests.
#[cfg(test)]
pub(crate) const READ_BACK: wgpu::BufferUsages = wgpu::BufferUsages::COPY_SRC;
#[cfg(not(test))]
pub(crate) const READ_BACK: wgpu::BufferUsages = wgpu::BufferUsages::empty();

/// Paths of the small layers; the tests set them per renderer to compare them in one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SmallLayerOptions {
    /// `RUSTCRAFT_OPT_PACKED_SHADER_CONSTANTS`.
    pub packed_shader_constants: bool,
    /// `RUSTCRAFT_OPT_SKIP_UNCHANGED_UNIFORMS`.
    pub skip_unchanged_uniforms: bool,
}

impl SmallLayerOptions {
    #[cfg(test)]
    pub const NEW: Self = SmallLayerOptions { packed_shader_constants: true, skip_unchanged_uniforms: true };
    /// The paths before.
    #[cfg(test)]
    pub const OLD: Self = SmallLayerOptions { packed_shader_constants: false, skip_unchanged_uniforms: false };

    pub fn from_switches() -> Self {
        SmallLayerOptions {
            packed_shader_constants: rc_data::opt_switch!("RUSTCRAFT_OPT_PACKED_SHADER_CONSTANTS"),
            skip_unchanged_uniforms: rc_data::opt_switch!("RUSTCRAFT_OPT_SKIP_UNCHANGED_UNIFORMS"),
        }
    }

    /// Shader `base` reduced to the blocks of these options (`//#if PACKED_SHADER_CONSTANTS`).
    pub fn shader_source(self, base: &str) -> String {
        crate::terrain_shader::select_blocks(base, |name| match name {
            "PACKED_SHADER_CONSTANTS" => self.packed_shader_constants,
            _ => panic!("small layer shader: unknown variant '{name}'"),
        })
    }
}

/// GPU buffer of a layer whose bytes go through the uploader only when they differ from what the
/// buffer already holds (`skip_unchanged`; without it, on every call: the path before).
pub(crate) struct BufferWrittenOnChange {
    buffer: FrameBuffer,
    /// Bytes at the start of the GPU buffer, as last written.
    on_gpu: Vec<u8>,
    skip_unchanged: bool,
    /// Test sabotage: "unchanged" judged on the length alone.
    #[cfg(test)]
    pub(crate) sabotage_length_only: bool,
}

impl BufferWrittenOnChange {
    /// Uniform of `size` bytes, created now: the bind groups read it before its first write.
    pub fn uniform(device: &wgpu::Device, label: &'static str, size: u64, skip_unchanged: bool) -> Self {
        let mut buffer = FrameBuffer::new(label, wgpu::BufferUsages::UNIFORM | READ_BACK, size);
        buffer.reserve(device, size);
        Self::over(buffer, skip_unchanged)
    }

    /// Vertex or instance buffer, created at its first write with `min_capacity` bytes at least.
    pub fn vertices(label: &'static str, min_capacity: u64, skip_unchanged: bool) -> Self {
        Self::over(FrameBuffer::new(label, wgpu::BufferUsages::VERTEX | READ_BACK, min_capacity), skip_unchanged)
    }

    fn over(buffer: FrameBuffer, skip_unchanged: bool) -> Self {
        BufferWrittenOnChange {
            buffer,
            on_gpu: Vec::new(),
            skip_unchanged,
            #[cfg(test)]
            sabotage_length_only: false,
        }
    }

    pub fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.buffer()
    }

    /// True if the start of the GPU buffer already equals `bytes`.
    fn holds(&self, bytes: &[u8]) -> bool {
        #[cfg(test)]
        if self.sabotage_length_only {
            return self.on_gpu.len() >= bytes.len();
        }
        self.on_gpu.starts_with(bytes)
    }

    /// Makes the start of the buffer equal to `bytes`; true if they were sent. A recreated buffer
    /// needs no care here: it is only recreated by a write, which sends everything.
    pub fn set(&mut self, up: &mut Uploader, queue: &wgpu::Queue, bytes: &[u8]) -> bool {
        if bytes.is_empty() || (self.skip_unchanged && self.holds(bytes)) {
            return false;
        }
        self.buffer.write(up, queue, bytes);
        self.on_gpu.clear();
        if self.skip_unchanged {
            self.on_gpu.extend_from_slice(bytes);
        }
        true
    }
}

#[cfg(test)]
#[path = "small_layers_test.rs"]
mod tests;
