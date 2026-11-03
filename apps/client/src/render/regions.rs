//! GPU buffers of the terrain shared by the columns of a region of 16 × 16 columns (faces, fluids and
//! translucent order in one buffer each, with the bind group that reads them), their growth, and the
//! identity buffer read by the indexed faces.

use wgpu::util::DeviceExt;

use super::upload::Uploader;
use super::{FACE_BYTES, ORDER_BYTES, REGION};
use crate::arena::Arena;

/// Region of 16 × 16 columns: one face buffer and one fluid buffer shared by its columns
/// (one binding and one buffer per region instead of one per column), origin in uniform.
pub(super) struct RegionGpu {
    pub(super) faces: wgpu::Buffer,
    pub(super) face_arena: Arena,
    pub(super) fluids: wgpu::Buffer,
    pub(super) fluid_arena: Arena,
    /// Draw order of the translucent quads: indices into `fluids`, re-sorted without re-sending the
    /// quads themselves (4 bytes instead of 108 or 128 per quad).
    pub(super) order: wgpu::Buffer,
    pub(super) order_arena: Arena,
    /// Origin of the region (uniform) and binding (origin + fluid quads read-only).
    pub(super) pos: wgpu::Buffer,
    pub(super) bind: wgpu::BindGroup,
    pub(super) columns: u32,
}

/// Usages of the region buffers (the fluids are also read by index in the shader).
const REGION_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::VERTEX
    .union(wgpu::BufferUsages::STORAGE)
    .union(wgpu::BufferUsages::COPY_DST)
    .union(wgpu::BufferUsages::COPY_SRC);

/// Range of `n` elements in a region's buffer, grown (GPU copy) if needed.
pub(super) fn alloc_in(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    up: &mut Uploader,
    buf: &mut wgpu::Buffer,
    arena: &mut Arena,
    n: u32,
    stride: u64,
) -> u32 {
    if let Some(s) = arena.alloc(n) {
        return s;
    }
    // Growth of 25%: little space wasted (the GPU copy only costs at loading).
    let cap = (arena.capacity + arena.capacity / 4).max(arena.capacity + n);
    let bigger = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("region"),
        size: u64::from(cap) * stride,
        usage: REGION_USAGE,
        mapped_at_creation: false,
    });
    // Copy submitted right away, preceded by the pending staging (writes into the old
    // buffer): the following writes into the new buffer come after.
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buf, 0, &bigger, 0, u64::from(arena.capacity) * stride);
    up.submit(queue, enc.finish());
    *buf = bigger;
    arena.grow(cap);
    arena.alloc(n).expect("room after growth")
}

impl RegionGpu {
    pub(super) fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        key: (i32, i32),
        fluid_bytes: u64,
        slots: &wgpu::Buffer,
    ) -> Self {
        let buffer = |label, n: u32, stride: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(n) * stride,
                usage: REGION_USAGE,
                mapped_at_creation: false,
            })
        };
        let (faces_cap, fluids_cap, order_cap) = (1 << 16, 1 << 10, 1 << 10);
        let pos = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[key.0 * REGION, key.1 * REGION, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let fluids = buffer("region fluids", fluids_cap, fluid_bytes);
        let faces = buffer("region faces", faces_cap, FACE_BYTES);
        let bind = region_bind(device, layout, &pos, &fluids, slots, &faces);
        RegionGpu {
            faces,
            face_arena: Arena::new(faces_cap),
            fluids,
            fluid_arena: Arena::new(fluids_cap),
            order: buffer("translucent order", order_cap, ORDER_BYTES),
            order_arena: Arena::new(order_cap),
            pos,
            bind,
            columns: 0,
        }
    }
}

pub(super) fn region_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    pos: &wgpu::Buffer,
    fluids: &wgpu::Buffer,
    slots: &wgpu::Buffer,
    faces: &wgpu::Buffer,
) -> wgpu::BindGroup {
    // The same quad buffer is read in the old format (1) or as compact translucents (2); the
    // faces are read by index in the indexed draws (4).
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: pos.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: fluids.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: fluids.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: slots.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: faces.as_entire_binding() },
        ],
    })
}

/// Identity buffer 0..n (u32), read as an instance attribute by the indexed faces.
pub(super) fn identity_buffer(device: &wgpu::Device, n: u32) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("face identity"),
        contents: bytemuck::cast_slice(&(0..n).collect::<Vec<u32>>()),
        usage: wgpu::BufferUsages::VERTEX,
    })
}
