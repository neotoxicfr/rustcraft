//! Frame captures (F2 and test captures): the copy of the displayed frame into a readback buffer, the wait
//! for the GPU, and the PNG or BMP file written from it.

use super::Renderer;
use crate::present_watch::Stage;

/// Maximum wait for the readback of a capture.
const READBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A capture waiting for its readback: destination file, readback buffer, bytes per row, width, height.
pub(super) type PendingCapture = (std::path::PathBuf, wgpu::Buffer, u32, u32, u32);

/// A world icon waiting for its readback, mapped without blocking the frame: the flag rises once
/// the mapping settled (mapped or failed), a later frame then hands the pixels to
/// `world_icon::save`.
pub(super) struct PendingWorldIcon {
    file: std::path::PathBuf,
    buffer: wgpu::Buffer,
    settled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    row: u32,
    side: u32,
}

/// Reason why the surface provided no frame (log, once per episode).
pub(super) fn surface_state(s: &wgpu::CurrentSurfaceTexture) -> &'static str {
    match s {
        wgpu::CurrentSurfaceTexture::Timeout => "timeout",
        wgpu::CurrentSurfaceTexture::Occluded => "window hidden",
        wgpu::CurrentSurfaceTexture::Validation => "validation error",
        _ => "other",
    }
}

/// 32-bit BMP, rows from top to bottom (negative height), channels B, G, R, A.
fn write_bmp(path: &std::path::Path, data: &[u8], row: usize, w: u32, h: u32, bgra: bool) -> std::io::Result<()> {
    let size = 54 + w as usize * h as usize * 4;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in 0..h as usize {
        for px in data[y * row..y * row + w as usize * 4].as_chunks::<4>().0 {
            if bgra {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            } else {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
    }
    std::fs::write(path, out)
}

impl Renderer {
    /// Records the next frame in `path` (32-bit BMP).
    pub fn request_capture(&mut self, path: std::path::PathBuf) {
        if self.can_capture {
            self.capture = Some(path);
        } else {
            eprintln!("capture impossible: the surface does not allow the copy");
        }
    }

    /// Records the copy of the frame into a readback buffer if a capture was requested; the file is
    /// written by `capture_readback` once the frame is submitted.
    pub(super) fn encode_capture_copy(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        frame_texture: &wgpu::Texture,
    ) -> Option<PendingCapture> {
        self.capture.take().map(|path| {
            let (w, h) = (self.config.width, self.config.height);
            let row = (w * 4).div_ceil(256) * 256;
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture"),
                size: u64::from(row * h),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                frame_texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            (path, buffer, row, w, h)
        })
    }

    /// Takes the world image of the next frame (before the interface) as the singleplayer world's
    /// icon `file` (`world_icon.rs`).
    pub fn request_world_icon(&mut self, file: std::path::PathBuf) {
        if self.can_capture && self.world_icon.is_none() {
            self.world_icon_request = Some(file);
        }
    }

    /// Records the copy of the centred square of the frame drawn so far (the world before the
    /// interface) into a readback buffer; only that square crosses to the CPU.
    pub(super) fn encode_world_icon_copy(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        frame_texture: &wgpu::Texture,
        file: std::path::PathBuf,
    ) {
        let (w, h) = (self.config.width, self.config.height);
        let side = w.min(h);
        let row = (side * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("world icon"),
            size: u64::from(row * side),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: (w - side) / 2, y: (h - side) / 2, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(side) },
            },
            wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
        );
        let settled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.world_icon = Some(PendingWorldIcon { file, buffer, settled, row, side });
    }

    /// Asks for the mapping of the icon copied by this frame (once it is submitted); the frame
    /// does not wait for it.
    pub(super) fn map_world_icon(&self) {
        if let Some(p) = &self.world_icon {
            let settled = p.settled.clone();
            p.buffer.slice(..).map_async(wgpu::MapMode::Read, move |_| {
                settled.store(true, std::sync::atomic::Ordering::Release);
            });
        }
    }

    /// Hands a mapped icon to its writer thread (`world_icon::save`) and frees its buffer; a
    /// failed mapping is dropped (the next attempt, a second later, takes another).
    pub(super) fn collect_world_icon(&mut self) {
        let settled = self.world_icon.as_ref().is_some_and(|p| p.settled.load(std::sync::atomic::Ordering::Acquire));
        let Some(p) = self.world_icon.take_if(|_| settled) else { return };
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let data = p.buffer.slice(..).get_mapped_range().map(|view| view.to_vec());
        match data {
            Ok(data) => crate::world_icon::save(p.file, data, p.row as usize, p.side, p.side, bgra),
            Err(e) => eprintln!("world icon {}: readback failed ({e})", p.file.display()),
        }
    }

    /// Waits (bounded) for the copy recorded by `encode_capture_copy` and writes the file: PNG for a
    /// player capture, BMP otherwise.
    pub(super) fn capture_readback(&self, pending: PendingCapture) {
        let (path, buffer, row, w, h) = pending;
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        // Bounded wait: a GPU that does not finish must not freeze the loop.
        let waited = crate::present_watch::during(Stage::Readback, || {
            self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(READBACK_TIMEOUT) })
        });
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        if let Err(e) = waited {
            eprintln!(
                "presentation blocked: capture {} abandoned, the GPU did not finish in {} s ({e})",
                path.display(),
                READBACK_TIMEOUT.as_secs()
            );
        } else {
            let data = slice.get_mapped_range().expect("capture read");
            // Player capture (F2): PNG written on another thread (`screenshot.rs`).
            if path.extension().is_some_and(|e| e == "png") {
                crate::screenshot::save(path, data.to_vec(), row as usize, w, h, bgra);
            } else {
                match write_bmp(&path, &data, row as usize, w, h, bgra) {
                    Ok(()) => eprintln!("capture: {}", path.display()),
                    Err(e) => eprintln!("capture {}: {e}", path.display()),
                }
            }
        }
    }
}
