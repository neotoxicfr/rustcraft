//! GPU objects of the terrain built once at startup: the render pipelines, the texture arrays of the
//! block textures, the table of baked faces, the layouts of the camera and column bind groups and the
//! formats of the instance buffers the shader reads.

use wgpu::util::DeviceExt;

use rc_assets::BlockModels;

use super::{DEPTH_NEARER, FACE_BYTES, FLUID96_WORDS, GpuFormats, ORDER_BYTES};
use crate::terrain_shader::ShaderOpts;

/// Baked face as read by the shader (96 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct QuadGpu {
    p: [[f32; 4]; 4],
    v: [f32; 4],
    info: [u32; 4],
}

/// Explicit face (fluid, translucent, offset model) as read by the shader (128 bytes):
/// corners, v, (texture | column in the region, 0, absolute column x, z), colour of each corner,
/// light of corners 0-1 and 2-3.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct FluidGpu {
    pub(super) p: [[f32; 4]; 4],
    pub(super) v: [f32; 4],
    pub(super) meta: [u32; 4],
    pub(super) colors: [u32; 4],
    pub(super) light: [u32; 4],
}

/// Instance buffer of each entry point of the terrain (`terrain.wgsl`).
pub(super) fn instance_layout(vs: &str) -> wgpu::VertexBufferLayout<'static> {
    const ORDER: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Uint32];
    const FACE: [wgpu::VertexAttribute; 5] =
        wgpu::vertex_attr_array![0 => Uint32, 1 => Uint32, 2 => Uint32, 3 => Uint32x2, 4 => Uint32];
    const FLUID_C: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32,
        6 => Uint32, 7 => Uint32x2
    ];
    const FLUID: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32x4,
        6 => Uint32x4, 7 => Uint32x4
    ];
    let (array_stride, attributes): (u64, &'static [wgpu::VertexAttribute]) = match vs {
        // Indexed faces: first face of the command, read from the identity buffer.
        "vs_sorted" | "vs_sorted_c" | "vs_idx" => (ORDER_BYTES, &ORDER),
        "vs" => (FACE_BYTES, &FACE),
        "vs_fluid_c" => (FLUID96_WORDS as u64 * 4, &FLUID_C),
        _ => (size_of::<FluidGpu>() as u64, &FLUID),
    };
    wgpu::VertexBufferLayout { array_stride, step_mode: wgpu::VertexStepMode::Instance, attributes }
}

pub(super) fn quad_gpu(q: &rc_assets::BakedQuad) -> QuadGpu {
    let mut p = [[0.0; 4]; 4];
    let mut v = [0.0; 4];
    for i in 0..4 {
        p[i] = [q.pos[i][0], q.pos[i][1], q.pos[i][2], q.uv[i][0]];
        v[i] = q.uv[i][1];
    }
    QuadGpu { p, v, info: [q.texture, q.tint as u32, q.shade_dir as u32, 0] }
}

pub(super) fn uniform_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let storage = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            storage(1),
            storage(2),
            storage(3),
            storage(4),
        ],
    })
}

/// Layout of the camera bindings (uniforms, atlases, instance buffers), common
/// to the terrain and to all the entity renderers. `lightmap_colors`: with the lightmap buffer of
/// `RUSTCRAFT_OPT_LIGHTMAPBUFFER` (binding 10), left out otherwise so that the old path keeps its
/// eight storage buffers per vertex stage.
pub(crate) fn camera_layout(device: &wgpu::Device, lightmap_colors: bool) -> wgpu::BindGroupLayout {
    let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
    let mut entries = vec![
        entry(
            0,
            wgpu::ShaderStages::VERTEX_FRAGMENT,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        ),
        vertex_storage(1),
        entry(
            2,
            wgpu::ShaderStages::FRAGMENT,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
        ),
        entry(3, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        entry(
            5,
            wgpu::ShaderStages::FRAGMENT,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
        ),
        entry(
            7,
            wgpu::ShaderStages::VERTEX,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        ),
        entry(8, wgpu::ShaderStages::VERTEX, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        vertex_storage(9),
        vertex_storage(6),
        vertex_storage(4),
    ];
    if lightmap_colors {
        entries.push(vertex_storage(10));
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &entries })
}

/// Read-only storage buffer of the vertex stage at `binding`.
fn vertex_storage(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Buffer of the baked faces of all the models (read by the shader).
pub(super) fn baked_faces_buffer(device: &wgpu::Device, models: &BlockModels) -> wgpu::Buffer {
    let quads: Vec<QuadGpu> = models.quads.iter().map(quad_gpu).collect();
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("baked faces"),
        contents: bytemuck::cast_slice(&quads),
        usage: wgpu::BufferUsages::STORAGE,
    })
}

/// A 2D texture array of block textures (mip levels computed at baking) written to the GPU: its view
/// and the bytes written.
pub(super) fn block_texture_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    size: u32,
    list: &[&Vec<rc_assets::texture::Image>],
) -> (wgpu::TextureView, u64) {
    let size = size.max(1);
    let mips = size.ilog2() + 1;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: list.len().max(1) as u32 },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Mip levels computed at baking, like the game (rc_assets::mip).
    let mut bytes = 0u64;
    for (layer, chain) in list.iter().enumerate() {
        for (level, data) in chain.iter().map(|i| &i.rgba).enumerate().take(mips as usize) {
            let s = (size >> level).max(1);
            bytes += data.len() as u64;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s * 4), rows_per_image: Some(s) },
                wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
            );
        }
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    (view, bytes)
}

/// Pipelines of the terrain: opaque, cutout and translucent block faces, the same three layers for
/// the fluids, the sorted translucents, and the sky.
pub(super) struct TerrainPipelines {
    pub(super) blocks: [wgpu::RenderPipeline; 3],
    pub(super) fluids: [wgpu::RenderPipeline; 3],
    pub(super) sorted: wgpu::RenderPipeline,
    pub(super) sky: wgpu::RenderPipeline,
}

/// Builds the terrain shader module and every terrain pipeline for a surface of `format`.
pub(super) fn terrain_pipelines(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    column_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    wire_mode: bool,
    shader_opts: ShaderOpts,
    formats: GpuFormats,
) -> TerrainPipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain"),
        source: wgpu::ShaderSource::Wgsl(crate::terrain_shader::terrain(shader_opts).into()),
    });
    crate::boot::mark("renderer: terrain shader module");
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(camera_layout), Some(column_layout)],
        ..Default::default()
    });
    let make = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>, depth_write: bool| {
        let instance_layout = instance_layout(vs);
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[Some(instance_layout)],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            }),
            // Back faces removed, like the game's terrain pipelines (water seen from below
            // has its own back face emitted by the mesher).
            primitive: wgpu::PrimitiveState {
                // Block faces as a 4-vertex strip (`RUSTCRAFT_OPT_STRIP`).
                topology: if vs == "vs" { shader_opts.topology() } else { wgpu::PrimitiveTopology::TriangleList },
                polygon_mode: if wire_mode { wgpu::PolygonMode::Line } else { wgpu::PolygonMode::Fill },
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(depth_write),
                // Ties accepted, write also for translucent: like the game ("≥" in
                // reversed depth).
                depth_compare: Some(DEPTH_NEARER),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let face_vs = shader_opts.face_entry();
    let pipelines = [
        make("opaque terrain", face_vs, "fs_solid", None, true),
        make("cutout terrain", face_vs, "fs_cutout", None, true),
        make("translucent terrain", face_vs, "fs_translucent", Some(wgpu::BlendState::ALPHA_BLENDING), true),
    ];
    let (vs_sorted, vs_fluid) = formats.entries();
    let sorted_pipeline =
        make("sorted translucent", vs_sorted, "fs_translucent", Some(wgpu::BlendState::ALPHA_BLENDING), true);
    let fluid_pipelines = [
        make("opaque fluid", vs_fluid, "fs_solid", None, true),
        make("cutout fluid", vs_fluid, "fs_cutout", None, true),
        make("translucent fluid", vs_fluid, "fs_translucent", Some(wgpu::BlendState::ALPHA_BLENDING), true),
    ];
    // Flat sky: fans generated in the shader, neither buffer nor depth.
    let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(camera_layout)],
        ..Default::default()
    });
    let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky"),
        layout: Some(&sky_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_sky"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_sky"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    TerrainPipelines { blocks: pipelines, fluids: fluid_pipelines, sorted: sorted_pipeline, sky: sky_pipeline }
}
