//! Full sky rendering (`docs/specs/sky.md`): sunrise/sunset glow, sun, moon,
//! stars (sky pass, before the terrain) and clouds (after the translucent terrain).
//!
//! Switches (read once at creation):
//! - `RUSTCRAFT_OPT_CLOUDINST`: `face` (default) one 4-byte instance per face, vertices drawn from the
//!   vertex number in the shader (like the game); `cell` one 8-byte instance per cell (up to
//!   4 faces, empty slots as degenerate triangles); `0` old mesh (96 bytes per face).
//! - `RUSTCRAFT_OPT_CLOUDBUF`: persistent clouds buffer rewritten through the common transit
//!   (worst case of the radius for the instances); `0` new buffer on every cell change.
//! - `RUSTCRAFT_OPT_CLOUDSPIRAL`: disk order precomputed per radius; `0` square sweep.
//! - `RUSTCRAFT_OPT_CLOUDPAR`: `1` faces or instances built in slices of the spiral on
//!   the common pool (with the spiral only); off by default (main thread alone) until the
//!   "flight" scene bench has measured the wait behind the queued meshes.
//! - `RUSTCRAFT_OPT_CLOUD_SPECULATIVE_BUILD`: (`face` format) a background thread builds the
//!   cell the camera will enter next; the crossing swaps buffers; `0` built at the crossing.
//! - `RUSTCRAFT_OPT_CLOUD_INDEXED_FACES`: (`face` format) 4 vertices per face drawn by a shared
//!   16-bit index list, faces read from storage by vertex number; `0` 6 vertices per instance.
//! - `RUSTCRAFT_OPT_PACKED_SHADER_CONSTANTS`: cube corners, shades and quad corners of the
//!   cloud shader as bit constants; `0` constant arrays copied and indexed per vertex.
//! - `RUSTCRAFT_OPT_SKYUNIF`: 7 uniforms in a buffer with 256-byte slots, one write of the
//!   range changed per frame, unchanged values skipped; `0` one buffer and one write each.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::render::upload::{self, Uploader};
use crate::sky_mesh::cloud_builder::{FaceBuilder, ParallelBuild, Speculation, Spiral, next_cell};
use crate::sky_mesh::{self, CloudKey, CloudMap, CloudSide};

/// Shape of the clouds buffer (`RUSTCRAFT_OPT_CLOUDINST`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudInst {
    Mesh,
    Face,
    Cell,
}

/// Sky switches, read once.
#[derive(Debug, Clone, Copy)]
pub struct SkyOpts {
    inst: CloudInst,
    persist: bool,
    spiral: bool,
    par: bool,
    unif: bool,
    speculative: bool,
    indexed: bool,
    packed_constants: bool,
}

impl SkyOpts {
    pub fn from_env() -> Self {
        SkyOpts {
            inst: match rc_data::env_str!("RUSTCRAFT_OPT_CLOUDINST") {
                Some("0") => CloudInst::Mesh,
                Some("cell") => CloudInst::Cell,
                _ => CloudInst::Face,
            },
            persist: rc_data::env_str!("RUSTCRAFT_OPT_CLOUDBUF") != Some("0"),
            spiral: rc_data::env_str!("RUSTCRAFT_OPT_CLOUDSPIRAL") != Some("0"),
            par: rc_data::env_str!("RUSTCRAFT_OPT_CLOUDPAR") == Some("1"),
            unif: rc_data::env_str!("RUSTCRAFT_OPT_SKYUNIF") != Some("0"),
            speculative: rc_data::opt_switch!("RUSTCRAFT_OPT_CLOUD_SPECULATIVE_BUILD"),
            indexed: rc_data::opt_switch!("RUSTCRAFT_OPT_CLOUD_INDEXED_FACES"),
            packed_constants: rc_data::opt_switch!("RUSTCRAFT_OPT_PACKED_SHADER_CONSTANTS"),
        }
    }

    /// Complete old path (each switch at its old value).
    #[cfg(test)]
    const OLD: SkyOpts = SkyOpts {
        inst: CloudInst::Mesh,
        persist: false,
        spiral: false,
        par: false,
        unif: false,
        speculative: false,
        indexed: false,
        packed_constants: false,
    };

    /// Faces drawn indexed from storage (only the `face` format has one word per face).
    fn indexed_faces(self) -> bool {
        self.indexed && self.inst == CloudInst::Face
    }

    /// Faces built ahead on a background thread (`face` format).
    fn speculative_faces(self) -> bool {
        self.speculative && self.inst == CloudInst::Face
    }

    /// Cloud shader entry point and its vertex or instance buffer (none for indexed faces).
    fn cloud_input(self) -> (&'static str, Option<wgpu::VertexBufferLayout<'static>>) {
        if self.indexed_faces() {
            return ("vs_cloud_indexed", None);
        }
        let (vs, buffer) = self.inst.vertex();
        (vs, Some(buffer))
    }

    /// Usage of the clouds buffer: also read from storage by the indexed faces.
    fn cloud_buffer_usage(self) -> wgpu::BufferUsages {
        if self.indexed_faces() { BUF_USAGE | wgpu::BufferUsages::STORAGE } else { BUF_USAGE }
    }

    /// sky.wgsl reduced to the blocks of these switches.
    pub(crate) fn shader_source(self) -> String {
        crate::terrain_shader::select_blocks(include_str!("sky.wgsl"), |name| match name {
            "PACKED_SHADER_CONSTANTS" => self.packed_constants,
            _ => panic!("sky.wgsl: unknown variant '{name}'"),
        })
    }
}

/// Sky uniforms: seven buffers (old path) or seven slots of one buffer.
enum Unif {
    Split(Vec<wgpu::Buffer>),
    Slots(Box<upload::Slots<7>>),
}

const MESH_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x4];
const FACE_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Uint32];
const CELL_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Sint16x2, 1 => Uint32];

impl CloudInst {
    /// Shader entry point and vertex or instance buffer.
    fn vertex(self) -> (&'static str, wgpu::VertexBufferLayout<'static>) {
        let (vs, array_stride, step_mode, attributes): (_, _, _, &'static [wgpu::VertexAttribute]) = match self {
            CloudInst::Mesh => ("vs_cloud", 16, wgpu::VertexStepMode::Vertex, &MESH_ATTRS),
            CloudInst::Face => ("vs_cloud_face", 4, wgpu::VertexStepMode::Instance, &FACE_ATTRS),
            CloudInst::Cell => ("vs_cloud_cell", 8, wgpu::VertexStepMode::Instance, &CELL_ATTRS),
        };
        (vs, wgpu::VertexBufferLayout { array_stride, step_mode, attributes })
    }
}

/// Clouds buffer: vertices or instances.
const BUF_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::VERTEX
    .union(wgpu::BufferUsages::COPY_DST)
    .union(if cfg!(test) { wgpu::BufferUsages::COPY_SRC } else { wgpu::BufferUsages::empty() });

/// Group 1 of the indexed faces: the clouds buffer read as storage by the vertices.
fn cloud_faces_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cloud faces"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

/// Sky images read from the resources: sun then 8 moon phases (32 × 32), clouds.
#[derive(Default)]
pub struct SkyAssets {
    /// Sun, 8 moon phases, End flash.
    pub celestial: Vec<rc_assets::texture::Image>,
    pub clouds: Option<rc_assets::texture::Image>,
    pub end_sky: Option<rc_assets::texture::Image>,
}

/// Parameters of the celestial bodies and clouds for a frame.
#[derive(Debug, Clone, Copy)]
pub struct SkyFrame {
    /// End sky to draw: (flash intensity, angle x, angle y).
    pub end: Option<(f32, f32, f32)>,
    pub sun_deg: f32,
    pub moon_deg: f32,
    pub star_deg: f32,
    pub star_brightness: f32,
    pub rain: f32,
    pub moon_phase: u32,
    pub sunrise: [f32; 4],
    pub clouds: Option<CloudFrame>,
}

#[derive(Debug, Clone, Copy)]
pub struct CloudFrame {
    /// Color already brought down to 8 bits per channel.
    pub color: [f32; 4],
    pub height: f32,
    pub game_time: i64,
    pub pt: f32,
    pub range_chunks: i32,
    pub cloud_end: f32,
    /// "Fast" clouds: one flat face per cell, top tint.
    pub flat: bool,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SkyUniform {
    mvp: [[f32; 4]; 4],
    color: [f32; 4],
    extra: [f32; 4],
}

/// Elements drawn (one uniform each).
const FAN: usize = 0;
const SUN: usize = 1;
const MOON: usize = 2;
const STARS: usize = 3;
const CLOUDS: usize = 4;
const END_BOX: usize = 5;
const END_FLASH: usize = 6;

pub struct Sky {
    fan_pipe: wgpu::RenderPipeline,
    body_pipe: wgpu::RenderPipeline,
    star_pipe: wgpu::RenderPipeline,
    cloud_pipe: wgpu::RenderPipeline,
    /// "Fast" clouds: flat layer seen from both sides (no face culling).
    flat_cloud_pipe: wgpu::RenderPipeline,
    /// Current "fast" (flat) mesh.
    cloud_flat: bool,
    end_pipe: wgpu::RenderPipeline,
    end_vb: wgpu::Buffer,
    has_end: bool,
    has_flash: bool,
    fan_vb: wgpu::Buffer,
    fan_count: u32,
    body_vb: wgpu::Buffer,
    star_vb: wgpu::Buffer,
    star_count: u32,
    opts: SkyOpts,
    /// Clouds buffer, its reserved size (persistent buffer), vertices per instance, instances.
    cloud_vb: Option<wgpu::Buffer>,
    cloud_cap: u64,
    cloud_verts: u32,
    cloud_n: u32,
    /// Indexed faces: shared index list, layout of group 1 and its binding of the clouds buffer.
    cloud_indices: Option<wgpu::Buffer>,
    cloud_faces_layout: wgpu::BindGroupLayout,
    cloud_faces_bind: Option<wgpu::BindGroup>,
    /// Precomputed disk order and its radius.
    spiral: (i32, Spiral),
    /// Faces of the last build and their builders (in place or ahead of time).
    face_builder: FaceBuilder,
    /// Where the in-place builds run.
    placement: ParallelBuild,
    /// Instances of the last build (reserve kept) and the slices of their parallel build.
    cells: Vec<[u32; 2]>,
    cell_parts: Vec<Vec<[u32; 2]>>,
    unif: Unif,
    binds: Vec<wgpu::BindGroup>,
    cloud_map: Option<Arc<CloudMap>>,
    cloud_key: Option<CloudKey>,
    /// Camera position in the cloud plane at the last frame (before the wrap), for its motion.
    cloud_eye: Option<(f64, f64)>,
    has_bodies: bool,
    /// What is to be drawn in the current frame (set by `prepare`).
    draw: [bool; 7],
    star_vb_len: u32,
    /// Layout of group 0, for the improved-transparency pipelines (made on demand).
    layout: wgpu::BindGroupLayout,
    /// Detailed and flat clouds, improved transparency.
    oit: Option<[crate::oit::Set; 2]>,
}

impl Sky {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, assets: &SkyAssets) -> Sky {
        Self::with_opts(device, queue, format, assets, SkyOpts::from_env())
    }

    fn with_opts(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        assets: &SkyAssets,
        opts: SkyOpts,
    ) -> Sky {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky"),
            source: wgpu::ShaderSource::Wgsl(opts.shader_source().into()),
        });
        // Celestial bodies: one layer per sprite (sun, moons, End flash) at the largest size,
        // enlarged with nearest neighbor (identical to the sampling of the original sprite).
        let layers = assets.celestial.len().max(1) as u32;
        let size = assets.celestial.iter().map(|i| i.width.max(i.height)).max().unwrap_or(1).max(1);
        let celestial = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("celestial bodies"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (layer, img) in assets.celestial.iter().enumerate() {
            let img = if img.width == size && img.height == size { img.clone() } else { img.resized(size) };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &celestial,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
        }
        let view = celestial.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler =
            device.create_sampler(&wgpu::SamplerDescriptor { label: Some("celestial bodies"), ..Default::default() });
        // End box: repeated texture, nearest neighbor.
        let end_img = assets.end_sky.clone().unwrap_or_else(|| rc_assets::texture::Image::missing(16));
        let end_tex = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("End sky"),
                size: wgpu::Extent3d { width: end_img.width, height: end_img.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &end_img.rgba,
        );
        let end_view = end_tex.create_view(&Default::default());
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("End sky"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // Compute: reread of the uniforms by the tests.
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT
                        | if cfg!(test) { wgpu::ShaderStages::COMPUTE } else { wgpu::ShaderStages::NONE },
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let unif = if opts.unif {
            Unif::Slots(Box::new(upload::Slots::new(device, ["sky"; 7], true, true)))
        } else {
            Unif::Split(
                (0..7)
                    .map(|_| {
                        device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("sky"),
                            size: size_of::<SkyUniform>() as u64,
                            usage: wgpu::BufferUsages::UNIFORM
                                | wgpu::BufferUsages::COPY_DST
                                | if cfg!(test) { wgpu::BufferUsages::COPY_SRC } else { wgpu::BufferUsages::empty() },
                            mapped_at_creation: false,
                        })
                    })
                    .collect(),
            )
        };
        let binds = (0..7)
            .map(|i| {
                let buf = match &unif {
                    Unif::Split(b) => b[i].as_entire_binding(),
                    Unif::Slots(s) => s.binding(i),
                };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: buf },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&end_view) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&repeat) },
                    ],
                })
            })
            .collect();
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let cloud_faces_layout = cloud_faces_layout(device);
        let cloud_pl = if opts.indexed_faces() {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("indexed clouds"),
                bind_group_layouts: &[Some(&layout), Some(&cloud_faces_layout)],
                ..Default::default()
            })
        } else {
            pl.clone()
        };
        let translucent = wgpu::BlendState::ALPHA_BLENDING;
        // Additive-α: source color × alpha added, source alpha kept.
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let make_cull = |label: &str,
                         (vs, buffer): (&str, Option<wgpu::VertexBufferLayout<'_>>),
                         fs: &str,
                         layout: &wgpu::PipelineLayout,
                         blend: wgpu::BlendState,
                         depth: bool,
                         cull: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: if buffer.is_some() { std::slice::from_ref(&buffer) } else { &[] },
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState { cull_mode: cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(depth),
                    depth_compare: Some(if depth {
                        crate::render::DEPTH_NEARER
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let make = |label: &str,
                    vs: &str,
                    fs: &str,
                    attributes: &[wgpu::VertexAttribute],
                    stride: u64,
                    blend: wgpu::BlendState,
                    depth: bool| {
            let buffer =
                wgpu::VertexBufferLayout { array_stride: stride, step_mode: wgpu::VertexStepMode::Vertex, attributes };
            make_cull(label, (vs, Some(buffer)), fs, &pl, blend, depth, Some(wgpu::Face::Back))
        };
        let fan_pipe =
            make("glow", "vs_fan", "fs_fan", &wgpu::vertex_attr_array![0 => Float32x4], 16, translucent, false);
        let body_pipe = make(
            "celestial bodies",
            "vs_body",
            "fs_body",
            &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            20,
            additive,
            false,
        );
        let star_pipe =
            make("stars", "vs_star", "fs_star", &wgpu::vertex_attr_array![0 => Float32x3], 12, additive, false);
        let back = Some(wgpu::Face::Back);
        let cloud_pipe = make_cull("clouds", opts.cloud_input(), "fs_cloud", &cloud_pl, translucent, true, back);
        let flat_cloud_pipe =
            make_cull("flat clouds", opts.cloud_input(), "fs_cloud", &cloud_pl, translucent, true, None);
        let end_pipe = make(
            "End sky",
            "vs_body",
            "fs_end",
            &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            20,
            translucent,
            false,
        );
        // End box: base quad (underside) rotated six times (§ 6.1).
        let base = [
            ([-100.0f32, -100.0, -100.0], [0.0f32, 0.0]),
            ([-100.0, -100.0, 100.0], [0.0, 16.0]),
            ([100.0, -100.0, 100.0], [16.0, 16.0]),
            ([100.0, -100.0, -100.0], [16.0, 0.0]),
        ];
        let half = std::f32::consts::FRAC_PI_2;
        let turns = [
            glam::Mat3::IDENTITY,
            glam::Mat3::from_rotation_x(half),
            glam::Mat3::from_rotation_x(-half),
            glam::Mat3::from_rotation_x(2.0 * half),
            glam::Mat3::from_rotation_z(half),
            glam::Mat3::from_rotation_z(-half),
        ];
        let mut end_box: Vec<[f32; 5]> = Vec::new();
        for m in turns {
            let v = base.map(|(p, uv)| {
                let q = m * Vec3::from(p);
                [q.x, q.y, q.z, uv[0], uv[1]]
            });
            end_box.extend([v[0], v[1], v[2], v[2], v[3], v[0]]);
        }
        let end_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("End sky"),
            contents: bytemuck::cast_slice(&end_box),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let fan = sky_mesh::sunrise_fan();
        let fan_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("glow"),
            contents: bytemuck::cast_slice(&fan),
            usage: wgpu::BufferUsages::VERTEX,
        });
        // Sun then moon quads (UVs rotated by 180° for the moon), two triangles each.
        let p = [[-1.0f32, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]];
        let sun_uv = [[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let moon_uv = [[1.0f32, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]];
        let quad = |uv: [[f32; 2]; 4]| [0, 1, 2, 2, 3, 0].map(|i| [p[i][0], p[i][1], p[i][2], uv[i][0], uv[i][1]]);
        let mut body: Vec<[f32; 5]> = quad(sun_uv).to_vec();
        body.extend(quad(moon_uv));
        let body_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("celestial bodies"),
            contents: bytemuck::cast_slice(&body),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let stars = sky_mesh::stars();
        let star_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("stars"),
            contents: bytemuck::cast_slice(&stars),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let cloud_map =
            assets.clouds.as_ref().map(|i| Arc::new(CloudMap::from_rgba(i.width as usize, i.height as usize, &i.rgba)));
        let speculation =
            cloud_map.as_ref().filter(|_| opts.speculative_faces()).and_then(|m| Speculation::start(Arc::clone(m)));
        Sky {
            fan_pipe,
            body_pipe,
            star_pipe,
            cloud_pipe,
            flat_cloud_pipe,
            cloud_flat: false,
            end_pipe,
            end_vb,
            has_end: assets.end_sky.is_some(),
            has_flash: assets.celestial.len() >= 10,
            fan_vb,
            fan_count: fan.len() as u32,
            body_vb,
            star_vb,
            star_count: stars.len() as u32,
            opts,
            cloud_vb: None,
            cloud_cap: 0,
            cloud_verts: 0,
            cloud_n: 0,
            cloud_indices: None,
            cloud_faces_layout,
            cloud_faces_bind: None,
            spiral: (-1, Spiral::from([])),
            face_builder: FaceBuilder::new(speculation),
            placement: ParallelBuild::new(opts.par),
            cells: Vec::new(),
            cell_parts: Vec::new(),
            unif,
            binds,
            cloud_map,
            cloud_key: None,
            cloud_eye: None,
            has_bodies: assets.celestial.len() >= 9,
            draw: [false; 7],
            star_vb_len: stars.len() as u32,
            layout,
            oit: None,
        }
    }

    /// Cloud pipelines for improved transparency (once).
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit.is_some() {
            return;
        }
        let source = self.opts.shader_source();
        let module = layouts.module(device, "clouds (transparency)", &source, include_str!("sky_oit.wgsl"));
        let groups = [Some(&self.layout), Some(&self.cloud_faces_layout)];
        let groups = if self.opts.indexed_faces() { &groups[..] } else { &groups[..1] };
        let (vs, buffer) = self.opts.cloud_input();
        let buffers = [buffer];
        let buffers = if buffers[0].is_some() { &buffers[..] } else { &[] };
        let spec = |label| crate::oit::Spec::new(label, groups, &module, vs, buffers, "fs_cloud");
        // Detailed clouds: back faces culled, bounds written into their own depth
        // (only the closest face counts afterwards); flat: two faces, no write.
        self.oit = Some([
            layouts.set(device, &spec("clouds (transparency)").cull(true).bounds_write()),
            layouts.set(device, &spec("flat clouds (transparency)")),
        ]);
    }

    /// Clouds are to be drawn in the frame.
    pub fn has_clouds(&self) -> bool {
        self.draw[CLOUDS] && self.cloud_n > 0
    }

    /// Buffer, vertices per instance and instances of the frame's clouds.
    fn cloud_draw(&self) -> Option<(&wgpu::Buffer, u32, u32)> {
        let vb = self.cloud_vb.as_ref().filter(|_| self.has_clouds())?;
        Some(match self.opts.inst {
            CloudInst::Mesh => (vb, self.cloud_n, 1),
            _ => (vb, self.cloud_verts, self.cloud_n),
        })
    }

    /// Clouds, stage `f` of improved transparency.
    pub fn draw_clouds_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        if let (Some(_), Some(sets)) = (self.cloud_draw(), &self.oit) {
            f.set(pass, &sets[usize::from(self.cloud_flat)]);
            self.record_cloud_draw(pass);
        }
    }

    /// Draw of the frame's clouds with the pipeline already set: indexed faces read from storage
    /// in draws of [`sky_mesh::FACES_PER_INDEXED_DRAW`] faces, or vertices and instances.
    fn record_cloud_draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some((vb, verts, n)) = self.cloud_draw() else { return };
        pass.set_bind_group(0, &self.binds[CLOUDS], &[]);
        if let (true, Some(indices), Some(faces)) =
            (self.opts.indexed_faces(), &self.cloud_indices, &self.cloud_faces_bind)
        {
            pass.set_bind_group(1, faces, &[]);
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint16);
            let per_draw = sky_mesh::FACES_PER_INDEXED_DRAW;
            for first in (0..n).step_by(per_draw as usize) {
                let count = (n - first).min(per_draw);
                // The base vertex carries the draw's first face: the shader reads face vertex / 4.
                pass.draw_indexed(0..6 * count, (4 * first) as i32, 0..1);
            }
            return;
        }
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.draw(0..verts, 0..n);
    }

    fn write(&mut self, queue: &wgpu::Queue, slot: usize, mvp: Mat4, color: [f32; 4], extra: [f32; 4]) {
        let u = SkyUniform { mvp: mvp.to_cols_array_2d(), color, extra };
        match &mut self.unif {
            Unif::Split(b) => queue.write_buffer(&b[slot], 0, bytemuck::bytes_of(&u)),
            Unif::Slots(s) => {
                let mut place = [0u8; upload::SLOT];
                place[..size_of::<SkyUniform>()].copy_from_slice(bytemuck::bytes_of(&u));
                s.set(slot, &place);
            }
        }
    }

    /// Rebuilds the clouds buffer for `key`, then asks for `next` (the cell the camera will most
    /// likely enter) to be built ahead.
    fn build_clouds(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        key: CloudKey,
        next: CloudKey,
    ) {
        let Some(map) = &self.cloud_map else { return };
        let mut timer = crate::perf_log::StageTimer::start();
        let inst = self.opts.inst;
        let radius = if inst == CloudInst::Mesh { key.radius } else { key.radius.min(sky_mesh::FACE_LIMIT) };
        let (key, next) = (CloudKey { radius, ..key }, CloudKey { radius, ..next });
        if self.opts.spiral && self.spiral.0 != radius {
            self.spiral = (radius, Spiral::from(sky_mesh::cloud_spiral(radius)));
        }
        let spiral = self.opts.spiral.then_some(&self.spiral.1);
        let mesh;
        let (bytes, n): (&[u8], u32) = match inst {
            CloudInst::Mesh => {
                mesh = sky_mesh::cloud_mesh(map, key.cx, key.cz, radius, key.side, key.flat);
                (bytemuck::cast_slice(&mesh), mesh.len() as u32)
            }
            CloudInst::Face => {
                if self.face_builder.build(map, key, spiral, next, &self.placement) {
                    crate::perf_log::counter_per_second("cloud_speculative_hits_per_second", 1);
                }
                let faces = &self.face_builder.faces;
                (bytemuck::cast_slice(faces), faces.len() as u32)
            }
            CloudInst::Cell => {
                let out = (&mut self.cells, &mut self.cell_parts);
                self.placement.run(sky_mesh::cloud_cell_instances, map, key, spiral, out);
                (bytemuck::cast_slice(&self.cells), self.cells.len() as u32)
            }
        };
        crate::perf_log::counter_per_second("cloud_builds_per_second", 1);
        timer.lap("us_cloud_build_per_frame");
        self.cloud_verts = match inst {
            CloudInst::Mesh | CloudInst::Face => 6,
            CloudInst::Cell if key.flat => 6,
            CloudInst::Cell => (6 * sky_mesh::CELL_FACES) as u32,
        };
        self.cloud_n = n;
        let usage = self.opts.cloud_buffer_usage();
        if !self.opts.persist {
            self.cloud_vb = (n > 0).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("clouds"),
                    contents: bytes,
                    usage: usage.difference(wgpu::BufferUsages::COPY_DST),
                })
            });
            self.bind_cloud_faces(device);
            return;
        }
        if n == 0 {
            return;
        }
        let len = bytes.len() as u64;
        let grows = len > self.cloud_cap;
        if grows {
            // Instances: worst case of the radius (4 faces per disk cell + inner faces),
            // reserved once; old mesh: power of 2 above.
            let cells = sky_mesh::disc_len(radius) as u64;
            let worst = match inst {
                CloudInst::Mesh => len.next_power_of_two(),
                CloudInst::Face => (cells * 4 + 54) * 4,
                CloudInst::Cell => (cells + 18) * 8,
            };
            self.cloud_cap = worst.max(len);
            self.cloud_vb = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("clouds"),
                size: self.cloud_cap,
                usage,
                mapped_at_creation: false,
            }));
        }
        up.write(queue, self.cloud_vb.as_ref().expect("clouds buffer"), 0, bytes);
        if grows {
            self.bind_cloud_faces(device);
        }
    }

    /// Indexed faces: the shared index list (made once) and the binding of the clouds buffer as
    /// storage (made again with each new buffer).
    fn bind_cloud_faces(&mut self, device: &wgpu::Device) {
        if !self.opts.indexed_faces() {
            return;
        }
        if self.cloud_indices.is_none() {
            self.cloud_indices = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cloud face indices"),
                contents: bytemuck::cast_slice(&sky_mesh::indexed_face_list(sky_mesh::QUAD_TRIANGLES)),
                usage: wgpu::BufferUsages::INDEX,
            }));
        }
        self.cloud_faces_bind = self.cloud_vb.as_ref().map(|vb| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cloud faces"),
                layout: &self.cloud_faces_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: vb.as_entire_binding() }],
            })
        });
    }

    /// Uniforms and mesh of the frame's clouds. `sky_vp`: projection × view without translation;
    /// `bodies`: sky pass allowed (Overworld); `eye`: camera.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        sky_vp: Mat4,
        f: &SkyFrame,
        bodies: bool,
        eye: [f64; 3],
    ) {
        let rad = |d: f32| d * (std::f32::consts::PI / 180.0);
        self.draw = [false; 7];
        if let Some((flash, ax, ay)) = f.end {
            if self.has_end {
                let c = 40.0 / 255.0;
                self.write(queue, END_BOX, sky_vp, [c, c, c, 1.0], [0.0; 4]);
                self.draw[END_BOX] = true;
            }
            if self.has_flash && flash > 1.0e-5 {
                let m = sky_vp
                    * Mat4::from_rotation_y(rad(180.0 - ay))
                    * Mat4::from_rotation_x(rad(-90.0 - ax))
                    * Mat4::from_translation(Vec3::new(0.0, 100.0, 0.0))
                    * Mat4::from_scale(Vec3::new(60.0, 1.0, 60.0));
                self.write(queue, END_FLASH, m, [flash, flash, flash, flash], [9.0, 0.0, 0.0, 0.0]);
                self.draw[END_FLASH] = true;
            }
        }
        if bodies {
            let c = f.sunrise;
            if c[3] > 0.001 {
                let phi = if sky_mesh::table_sin(rad(f.sun_deg)) < 0.0 { 180.0 } else { 0.0 };
                let m = sky_vp
                    * Mat4::from_rotation_x(rad(90.0))
                    * Mat4::from_rotation_z(rad(phi + 90.0))
                    * Mat4::from_scale(Vec3::new(1.0, 1.0, c[3]));
                self.write(queue, FAN, m, c, [0.0; 4]);
                self.draw[FAN] = true;
            }
            if self.has_bodies {
                let lum = 1.0 - f.rain;
                let body = |angle: f32, half: f32| {
                    sky_vp
                        * Mat4::from_rotation_y(rad(-90.0))
                        * Mat4::from_rotation_x(rad(angle))
                        * Mat4::from_translation(Vec3::new(0.0, 100.0, 0.0))
                        * Mat4::from_scale(Vec3::new(half, 1.0, half))
                };
                self.write(queue, SUN, body(f.sun_deg, 30.0), [1.0, 1.0, 1.0, lum], [0.0; 4]);
                self.write(
                    queue,
                    MOON,
                    body(f.moon_deg, 20.0),
                    [1.0, 1.0, 1.0, lum],
                    [1.0 + f.moon_phase.min(7) as f32, 0.0, 0.0, 0.0],
                );
                self.draw[SUN] = true;
                self.draw[MOON] = true;
            }
            let b = f.star_brightness;
            if b > 0.0 {
                let m = sky_vp * Mat4::from_rotation_y(rad(-90.0)) * Mat4::from_rotation_x(rad(f.star_deg));
                self.write(queue, STARS, m, [b, b, b, b], [0.0; 4]);
                self.draw[STARS] = true;
            }
        }
        if let (Some(c), Some(map_size)) = (f.clouds, self.cloud_map.as_ref().map(|m| m.size))
            && c.color[3] > 0.0
        {
            let radius = sky_mesh::cloud_radius(c.range_chunks);
            let low = (f64::from(c.height) - eye[1]) as f32;
            let side = if low + 4.0 < 0.0 {
                CloudSide::Above
            } else if low > 0.0 {
                CloudSide::Below
            } else {
                CloudSide::Inside
            };
            let offset = (c.game_time.rem_euclid(102_400)) as f32 + c.pt;
            let size = (map_size * 12) as f64;
            let unwrapped = (eye[0] + f64::from(offset * 0.030_000_001), eye[2] + 3.96);
            let motion = self.cloud_eye.map_or((0.0, 0.0), |(x, z)| (unwrapped.0 - x, unwrapped.1 - z));
            self.cloud_eye = Some(unwrapped);
            let (mut nx, mut nz) = unwrapped;
            nx -= (nx / size).floor() * size;
            nz -= (nz / size).floor() * size;
            let (cx, cz) = ((nx / 12.0).floor() as i32, (nz / 12.0).floor() as i32);
            let inside = (nx - 12.0 * f64::from(cx), nz - 12.0 * f64::from(cz));
            let (fx, fz) = (inside.0 as f32, inside.1 as f32);
            let key = CloudKey { cx, cz, side, radius, flat: c.flat };
            if self.cloud_key != Some(key) {
                let (next_x, next_z) = next_cell((cx, cz), inside, motion, map_size as i32);
                self.build_clouds(device, queue, up, key, CloudKey { cx: next_x, cz: next_z, ..key });
                self.cloud_key = Some(key);
                self.cloud_flat = c.flat;
            } else {
                self.face_builder.poll();
            }
            self.write(queue, CLOUDS, sky_vp, c.color, [-fx, low, -fz, c.cloud_end]);
            self.draw[CLOUDS] = self.cloud_n > 0;
        }
        if let Unif::Slots(s) = &mut self.unif {
            s.flush(up, queue);
        }
    }

    /// End sky: box, then flash (in place of the dome and the bodies).
    pub fn draw_end(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.draw[END_BOX] {
            pass.set_pipeline(&self.end_pipe);
            pass.set_bind_group(0, &self.binds[END_BOX], &[]);
            pass.set_vertex_buffer(0, self.end_vb.slice(..));
            pass.draw(0..36, 0..1);
        }
        if self.draw[END_FLASH] {
            pass.set_pipeline(&self.body_pipe);
            pass.set_vertex_buffer(0, self.body_vb.slice(..));
            pass.set_bind_group(0, &self.binds[END_FLASH], &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// Glow, sun, moon, stars (after the dome, before the dark disk).
    pub fn draw_bodies(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.draw[FAN] {
            pass.set_pipeline(&self.fan_pipe);
            pass.set_bind_group(0, &self.binds[FAN], &[]);
            pass.set_vertex_buffer(0, self.fan_vb.slice(..));
            pass.draw(0..self.fan_count, 0..1);
        }
        if self.draw[SUN] {
            pass.set_pipeline(&self.body_pipe);
            pass.set_vertex_buffer(0, self.body_vb.slice(..));
            pass.set_bind_group(0, &self.binds[SUN], &[]);
            pass.draw(0..6, 0..1);
            pass.set_bind_group(0, &self.binds[MOON], &[]);
            pass.draw(6..12, 0..1);
        }
        if self.draw[STARS] {
            pass.set_pipeline(&self.star_pipe);
            pass.set_bind_group(0, &self.binds[STARS], &[]);
            pass.set_vertex_buffer(0, self.star_vb.slice(..));
            pass.draw(0..self.star_count.min(self.star_vb_len), 0..1);
        }
    }

    /// Clouds (after the translucent terrain), from the closest to the farthest.
    pub fn draw_clouds(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.cloud_draw().is_some() {
            pass.set_pipeline(if self.cloud_flat { &self.flat_cloud_pipe } else { &self.cloud_pipe });
            self.record_cloud_draw(pass);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Proofs on a real GPU (ignored without a GPU, failure under `RUSTCRAFT_OPT_REQUIRE=1`):
    //! - the decoding of the faces and cells of sky.wgsl, run by a compute shader, returns
    //!   bit for bit the vertices of the CPU emulator (itself equal to the old mesh, see
    //!   `sky_mesh`);
    //! - a frame script (day, dawn, night, End, clouds above, inside, below, flat,
    //!   camera crossing cells) played by the old path and by each new
    //!   combination: drawn uniforms and clouds buffer read back and compared.
    use super::*;
    use crate::render::upload::tests::{gpu, read};
    use crate::sky_mesh::{
        CloudCells, cloud_cell_instances, cloud_faces, cloud_mesh, expand_cells, face_corner, face_vertex,
    };

    /// Every new path on; format `face`, parallel build on the common pool off.
    const NEW: SkyOpts = SkyOpts {
        inst: CloudInst::Face,
        persist: true,
        spiral: true,
        par: false,
        unif: true,
        speculative: true,
        indexed: true,
        packed_constants: true,
    };

    fn test_cloud_image() -> rc_assets::texture::Image {
        let mut s = 0x9e37_79b9_7f4a_7c15u64;
        let mut rgba = vec![0u8; 64 * 64 * 4];
        for px in rgba.chunks_mut(4) {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            px[3] = if s % 100 < 50 { 255 } else { 0 };
        }
        rc_assets::texture::Image { width: 64, height: 64, rgba }
    }

    const PROBE: &str = r"
@group(0) @binding(10) var<storage, read> t_faces: array<u32>;
@group(0) @binding(11) var<storage, read> t_cells: array<vec2<u32>>;
@group(0) @binding(12) var<storage, read_write> t_out: array<vec4<f32>>;

@compute @workgroup_size(64)
fn t_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let nf = arrayLength(&t_faces) * 6u;
    let nc = arrayLength(&t_cells) * 24u;
    let v = id.x;
    if v < nf {
        t_out[v] = cloud_face(t_faces[v / 6u], v % 6u);
        return;
    }
    let c = v - nf;
    if c < nc {
        let ik = t_cells[c / 24u].x;
        t_out[v] = cloud_cell(bitcast<i32>(ik << 16u) >> 16u, bitcast<i32>(ik) >> 16u, t_cells[c / 24u].y, c % 24u);
        return;
    }
    // Indexed faces: vertex 4 × face + corner, as vs_cloud_indexed reads it.
    let q = c - nc;
    if q < arrayLength(&t_faces) * 4u {
        t_out[v] = cloud_word_corner(t_faces[q / 4u], q & 3u);
    }
}
";

    fn storage(device: &wgpu::Device, data: &[u8], out: bool) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: data,
            usage: wgpu::BufferUsages::STORAGE
                | if out { wgpu::BufferUsages::COPY_SRC } else { wgpu::BufferUsages::empty() },
        })
    }

    /// Instanced faces, cells and indexed faces decoded by the shader (constant arrays and packed
    /// constants alike) return bit for bit the vertices of the CPU emulator.
    #[test]
    fn cloud_shader_decodes_like_the_emulator() {
        let Some((device, queue)) = gpu() else { return };
        for packed_constants in [false, true] {
            decode_on_the_gpu(&device, &queue, SkyOpts { packed_constants, ..NEW });
        }
    }

    fn decode_on_the_gpu(device: &wgpu::Device, queue: &wgpu::Queue, opts: SkyOpts) {
        let img = test_cloud_image();
        let map = CloudMap::from_rgba(64, 64, &img.rgba);
        let (mut faces, mut cells) = (Vec::new(), Vec::new());
        let (mut f, mut c) = (Vec::new(), Vec::new());
        for (cx, cz, r) in [(0, 0, 12), (-40, 77, 30), (5, 5, 1)] {
            for side in [CloudSide::Above, CloudSide::Inside, CloudSide::Below] {
                for flat in [false, true] {
                    cloud_faces(&map, cx, cz, CloudCells::Scan(r), side, flat, &mut f);
                    faces.extend_from_slice(&f);
                    cloud_cell_instances(&map, cx, cz, CloudCells::Scan(r), side, flat, &mut c);
                    cells.extend_from_slice(&c);
                }
            }
        }
        // Extreme corners of the packing.
        for (i, k) in [(-511, 511), (511, -511), (-1, -1)] {
            faces.push(sky_mesh::pack_face(i, k, 4 | sky_mesh::FACE_INSIDE));
            cells.push([u32::from(i as u16) | u32::from(k as u16) << 16, 0xff_ff_10_0b]);
        }
        let mut want = sky_mesh::expand_faces(&faces);
        for &[ik, codes] in &cells {
            let (i, k) = (i32::from(ik as u16 as i16), i32::from((ik >> 16) as u16 as i16));
            for v in 0..24 {
                let code = (codes >> (8 * (v / 6))) & 0xff;
                want.push(if code == 0xff { [0.0, 0.0, 0.0, -1.0] } else { face_vertex(i, k, code, v % 6) });
            }
        }
        let sext = |x: u32| ((x << 22) as i32) >> 22;
        for &w in &faces {
            want.extend((0..4).map(|corner| face_corner(sext(w), sext(w >> 10), (w >> 20) & 31, corner)));
        }
        // Inner faces are among them, and reversing their corners moves every corner: a shader
        // that forgot the reversal would differ here.
        let inner = faces.iter().filter(|&&w| (w >> 20) & sky_mesh::FACE_INSIDE != 0).count();
        assert!(inner > 50, "{inner} inner faces decoded");
        for dir in 0..6 {
            let inside = dir | sky_mesh::FACE_INSIDE;
            assert!((0..4).all(|c| face_corner(3, -2, inside, c) != face_corner(3, -2, dir, c)), "direction {dir}");
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{PROBE}", opts.shader_source()).into()),
        });
        let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("t_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let fb = storage(device, bytemuck::cast_slice(&faces), false);
        let cb = storage(device, bytemuck::cast_slice(&cells), false);
        let ob = storage(device, &vec![0u8; want.len() * 16], true);
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 10, resource: fb.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 11, resource: cb.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 12, resource: ob.as_entire_binding() },
            ],
        });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups((want.len() as u32).div_ceil(64), 1, 1);
        }
        queue.submit([enc.finish()]);
        let got: Vec<[f32; 4]> = bytemuck::cast_slice(&read(device, queue, &ob)).to_vec();
        let bits = |v: &[[f32; 4]]| v.iter().flatten().map(|x| x.to_bits()).collect::<Vec<_>>();
        let first = bits(&got).iter().zip(bits(&want)).position(|(a, b)| *a != b);
        assert!(
            first.is_none(),
            "packed constants {}: vertex {:?}: {:?} instead of {:?}",
            opts.packed_constants,
            first,
            got[first.unwrap_or(0) / 4],
            want[first.unwrap_or(0) / 4]
        );
    }

    /// Both constant variants (alone and with the improved-transparency entries) are valid WGSL.
    #[test]
    fn sky_shader_variants_are_valid() {
        for packed_constants in [false, true] {
            let src = SkyOpts { packed_constants, ..NEW }.shader_source();
            let (m, _) = crate::terrain_shader::tests::validate(&src);
            for e in ["vs_cloud", "vs_cloud_face", "vs_cloud_cell", "vs_cloud_indexed", "fs_cloud"] {
                assert!(m.entry_points.iter().any(|p| p.name == e), "{e} absent");
            }
            crate::terrain_shader::tests::validate(&format!(
                "{src}\n{}\n{}",
                crate::oit::Options::NEW.common_source(),
                include_str!("sky_oit.wgsl")
            ));
        }
    }

    /// Packed constants: no array held by value and indexed in the cloud vertices; the constant
    /// arrays have some (the check sees what it looks for).
    #[test]
    fn packed_cloud_vertices_index_no_value_array() {
        let count = |packed_constants: bool| {
            let src = SkyOpts { packed_constants, ..NEW }.shader_source();
            let (m, info) = crate::terrain_shader::tests::validate(&src);
            let cloud_vertex = |name: &str| ["vs_cloud_face", "vs_cloud_cell", "vs_cloud_indexed"].contains(&name);
            crate::terrain_shader::tests::vertex_value_array_indexing(&m, &info, cloud_vertex)
        };
        assert_eq!(count(true), 0, "array indexed by value with the packed constants");
        assert!(count(false) >= 3, "indexed constant arrays not seen: {}", count(false));
    }

    fn assets() -> SkyAssets {
        let img = |n: u8| rc_assets::texture::Image { width: 2, height: 2, rgba: vec![n; 16] };
        SkyAssets { celestial: (0..10).map(img).collect(), clouds: Some(test_cloud_image()), end_sky: Some(img(40)) }
    }

    fn frame(t: f32, clouds: Option<CloudFrame>, end: bool) -> SkyFrame {
        SkyFrame {
            end: end.then_some((0.5 + t, 10.0 * t, 20.0 * t)),
            sun_deg: 360.0 * t,
            moon_deg: 360.0 * t + 180.0,
            star_deg: 90.0 * t,
            star_brightness: (t - 0.5).max(0.0),
            rain: 0.25,
            moon_phase: (t * 8.0) as u32,
            sunrise: [1.0, 0.5, 0.2, (0.3 - t).max(0.0)],
            clouds,
        }
    }

    const UNIF_PROBE: &str = r"
@group(1) @binding(1) var<storage, read_write> t_u: array<vec4<f32>, 6>;

@compute @workgroup_size(1)
fn t_unif() {
    t_u[0] = u.mvp[0];
    t_u[1] = u.mvp[1];
    t_u[2] = u.mvp[2];
    t_u[3] = u.mvp[3];
    t_u[4] = u.color;
    t_u[5] = u.extra;
}
";

    /// Reading of the uniform seen by the shader through a sky binding group.
    struct UnifProbe {
        pipe: wgpu::ComputePipeline,
        out: wgpu::Buffer,
        bind: wgpu::BindGroup,
    }

    impl UnifProbe {
        fn new(device: &wgpu::Device, sky: &Sky) -> Self {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(format!("{}\n{UNIF_PROBE}", sky.opts.shader_source()).into()),
            });
            // Binding 1: group 1 binding 0 is the storage of the indexed cloud faces.
            let out_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&sky.layout), Some(&out_layout)],
                ..Default::default()
            });
            let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: Some(&layout),
                module: &module,
                entry_point: Some("t_unif"),
                compilation_options: Default::default(),
                cache: None,
            });
            let out = storage(device, &[0u8; 96], true);
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &out_layout,
                entries: &[wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() }],
            });
            UnifProbe { pipe, out, bind }
        }

        fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue, group: &wgpu::BindGroup) -> Vec<u8> {
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.pipe);
                pass.set_bind_group(0, group, &[]);
                pass.set_bind_group(1, &self.bind, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            queue.submit([enc.finish()]);
            read(device, queue, &self.out)
        }
    }

    /// Drawn uniforms (read by the shader) and clouds vertices of a frame.
    type Shot = (Vec<Vec<u8>>, Vec<[f32; 4]>);

    /// Snapshot of the frame, read back from the GPU.
    fn snapshot(sky: &Sky, probe: &UnifProbe, device: &wgpu::Device, queue: &wgpu::Queue) -> Shot {
        let unif = (0..7).filter(|&i| sky.draw[i]).map(|i| probe.read(device, queue, &sky.binds[i])).collect();
        let verts = match sky.cloud_draw() {
            None => Vec::new(),
            Some((vb, verts, n)) => {
                let bytes = read(device, queue, vb);
                match sky.opts.inst {
                    CloudInst::Mesh => bytemuck::cast_slice::<u8, [f32; 4]>(&bytes)[..verts as usize].to_vec(),
                    CloudInst::Face => sky_mesh::expand_faces(&bytemuck::cast_slice::<u8, u32>(&bytes)[..n as usize]),
                    CloudInst::Cell => expand_cells(&bytemuck::cast_slice::<u8, [u32; 2]>(&bytes)[..n as usize]),
                }
            }
        };
        (unif, verts)
    }

    /// Plays the script; returns, per frame, the snapshot and the number of transit writes.
    fn play(device: &wgpu::Device, queue: &wgpu::Queue, opts: SkyOpts) -> Vec<(Shot, u64)> {
        let mut sky = Sky::with_opts(device, queue, wgpu::TextureFormat::Rgba8Unorm, &assets(), opts);
        let mut up = Uploader::new(device, true);
        let probe = UnifProbe::new(device, &sky);
        let mut out = Vec::new();
        for n in 0..48 {
            let t = n as f32 / 48.0;
            let clouds = (n % 12 != 11).then_some(CloudFrame {
                color: [1.0, 1.0, 1.0, 0.8],
                height: 192.33,
                game_time: 1000 + 37 * n as i64,
                pt: 0.25,
                range_chunks: [4, 4, 8, 2][n / 12],
                cloud_end: 100.0,
                flat: n % 7 == 3,
            });
            // Camera: below, in the layer, above; crosses cells of 12 blocks.
            let eye = [-300.0 + 7.5 * n as f64, [150.0, 190.0, 194.0, 250.0][n % 4], 20.0 - 5.0 * n as f64];
            let vp = Mat4::from_scale(Vec3::new(0.01, 0.02, 0.005)) * Mat4::from_rotation_y(t * 6.0);
            let end = (36..42).contains(&n);
            sky.prepare(device, queue, &mut up, vp, &frame(t, clouds, end), !end, eye);
            let writes = up.take_stats().writes;
            up.submit(queue, device.create_command_encoder(&Default::default()).finish());
            out.push((snapshot(&sky, &probe, device, queue), writes));
        }
        out
    }

    #[test]
    fn sky_buffers_match_the_old_path() {
        let Some((device, queue)) = gpu() else { return };
        let reference = play(&device, &queue, SkyOpts::OLD);
        assert!(reference.iter().all(|(_, w)| *w == 0), "the old path writes through the queue");
        assert!(reference.iter().filter(|((_, v), _)| !v.is_empty()).count() > 30, "clouds drawn");
        for inst in [CloudInst::Mesh, CloudInst::Face, CloudInst::Cell] {
            let old = SkyOpts { inst, ..SkyOpts::OLD };
            let new = SkyOpts { inst, ..NEW };
            for opts in [
                SkyOpts { persist: true, spiral: true, par: true, unif: true, ..old },
                SkyOpts { persist: true, spiral: true, unif: true, ..old },
                SkyOpts { unif: true, ..old },
                SkyOpts { persist: true, ..old },
                new,
                SkyOpts { par: true, ..new },
                SkyOpts { speculative: false, ..new },
                SkyOpts { spiral: false, ..new },
                SkyOpts { persist: false, ..new },
            ] {
                let got = play(&device, &queue, opts);
                for (n, (((u, v), w), ((ru, rv), _))) in got.iter().zip(&reference).enumerate() {
                    assert!(u == ru, "uniforms frame {n} {opts:?}");
                    assert!(v == rv, "clouds frame {n} {opts:?}");
                    // At most one uniform write, plus the clouds buffer if it is redone.
                    let max = u64::from(opts.unif) + u64::from(opts.persist);
                    assert!(*w <= max, "writes frame {n} {opts:?}: {w}");
                }
            }
        }
        // Sensitive comparison: a mesh of another center differs.
        let img = test_cloud_image();
        let map = CloudMap::from_rgba(64, 64, &img.rgba);
        assert_ne!(
            cloud_mesh(&map, 0, 0, 6, CloudSide::Below, false),
            cloud_mesh(&map, 1, 0, 6, CloudSide::Below, false)
        );
    }

    /// The parallel build stays off without `RUSTCRAFT_OPT_CLOUDPAR=1`.
    #[test]
    fn cloud_par_is_off_by_default() {
        if !rc_data::env_flag!("RUSTCRAFT_OPT_CLOUDPAR") {
            assert!(!SkyOpts::from_env().par);
        }
    }

    /// Speculation, indexed faces and packed constants are on unless set to 0.
    #[test]
    fn cloud_switches_have_their_measured_defaults() {
        let o = SkyOpts::from_env();
        assert!(o.speculative || rc_data::env_flag!("RUSTCRAFT_OPT_CLOUD_SPECULATIVE_BUILD"));
        assert!(o.indexed || rc_data::env_flag!("RUSTCRAFT_OPT_CLOUD_INDEXED_FACES"));
        assert!(o.packed_constants || rc_data::env_flag!("RUSTCRAFT_OPT_PACKED_SHADER_CONSTANTS"));
    }

    mod cloud_gpu_bench;

    const SIZE: u32 = 64;

    /// Views of the render test: eye (height), direction, "fast" mode, range in chunks. The last
    /// one, toward the horizon at range 128 with the fade pushed back, needs several indexed draws.
    const VIEWS: [(f64, [f32; 3], bool, i32); 7] = [
        (150.0, [0.3, 0.6, 1.0], false, 6),
        (190.0, [1.0, 0.1, 0.4], false, 6),
        (194.0, [-0.2, 0.05, 1.0], false, 6),
        (250.0, [0.4, -0.7, 0.8], false, 6),
        (150.0, [-0.5, 0.5, 0.6], true, 6),
        (250.0, [0.1, -0.8, -0.5], true, 6),
        (150.0, [0.2, 0.12, 1.0], false, 128),
    ];

    /// Cloud images (64 × 64 RGBA) of each view, drawn as in the client: classic pass,
    /// or improved-transparency stages then composition (`oit`).
    fn render_clouds(device: &wgpu::Device, queue: &wgpu::Queue, opts: SkyOpts, oit: bool) -> Vec<Vec<u8>> {
        render_clouds_with(device, queue, opts, oit.then(crate::oit::Options::from_switches), sky_mesh::QUAD_TRIANGLES)
    }

    /// [`render_clouds`] with the indexed faces drawn by the corner order `triangles` (sabotage),
    /// improved transparency in the form `oit`.
    fn render_clouds_with(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        opts: SkyOpts,
        oit: Option<crate::oit::Options>,
        triangles: [u16; 6],
    ) -> Vec<Vec<u8>> {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut sky = Sky::with_opts(device, queue, format, &assets(), opts);
        if triangles != sky_mesh::QUAD_TRIANGLES {
            sky.cloud_indices = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&sky_mesh::indexed_face_list(triangles)),
                usage: wgpu::BufferUsages::INDEX,
            }));
        }
        let mut up = Uploader::new(device, true);
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                view_formats: &[],
            })
        };
        let color = target(format, wgpu::TextureUsages::COPY_SRC);
        let color_view = color.create_view(&Default::default());
        let depth = target(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::TEXTURE_BINDING)
            .create_view(&Default::default());
        let mut oit = oit.map(|options| {
            let mut o = crate::oit::Oit::with_options(device, format, options);
            o.ensure(device, (SIZE, SIZE), &depth);
            sky.ensure_oit(device, &o.layouts);
            o
        });
        let proj = crate::render::perspective(70.0, 1.0, 0.05, 2000.0);
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let mut images = Vec::new();
        for (n, (y, dir, flat, range_chunks)) in VIEWS.into_iter().enumerate() {
            let vp = proj * glam::camera::rh::view::look_to_mat4(Vec3::ZERO, Vec3::from(dir).normalize(), Vec3::Y);
            let clouds = CloudFrame {
                color: [1.0, 0.9, 0.8, 0.8],
                height: 192.33,
                game_time: 1000 + 997 * n as i64,
                pt: 0.25,
                range_chunks,
                cloud_end: if range_chunks > 6 { 2048.0 } else { 110.0 },
                flat,
            };
            let eye = [13.0 * n as f64 - 20.0, y, 5.5 - 9.0 * n as f64];
            sky.prepare(device, queue, &mut up, vp, &frame(0.3, Some(clouds), false), false, eye);
            if let Some(o) = &mut oit {
                o.set_projection(&mut up, queue, proj);
            }
            up.submit(queue, device.create_command_encoder(&Default::default()).finish());
            assert!(sky.has_clouds(), "clouds in view {n}");
            if opts.inst == CloudInst::Face && range_chunks > 6 {
                let faces = sky.cloud_n;
                assert!(faces > 2 * sky_mesh::FACES_PER_INDEXED_DRAW, "view {n}: {faces} faces, a single indexed draw");
            }
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.2, g: 0.4, b: 0.9, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::render::DEPTH_CLEAR),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                if oit.is_none() {
                    sky.draw_clouds(&mut pass);
                }
            }
            if let Some(o) = &oit {
                use crate::oit::Stage;
                for stage in Stage::ALL {
                    drop(o.begin(&mut enc, stage, &depth, true, "clear", None));
                    if stage == Stage::Bounds {
                        o.cull(&mut enc, &depth, None);
                    }
                    let mut pass = o.begin_clouds(&mut enc, stage, None);
                    sky.draw_clouds_oit(&mut pass, o.frame(stage));
                }
                o.composite(&mut enc, &color_view, &depth, None);
            }
            enc.copy_texture_to_buffer(
                color.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &out,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIZE * 4),
                        rows_per_image: Some(SIZE),
                    },
                },
                wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            );
            queue.submit([enc.finish()]);
            images.push(read(device, queue, &out));
        }
        images
    }

    /// Real draw (instanced draw, `Uint32` and `Sint16x2` attributes, indexed faces read from
    /// storage, constant arrays or packed constants, classic and improved-transparency pipelines):
    /// every variant of the `face` and `cell` formats returns the old mesh's image pixel for
    /// pixel, below, inside, above and in "fast" mode. Sabotage: the indexed triangles wound the
    /// other way change the image.
    #[test]
    fn packed_clouds_render_like_the_mesh() {
        let Some((device, queue)) = gpu() else { return };
        let old = SkyOpts { persist: true, spiral: true, unif: true, ..SkyOpts::OLD };
        let variants = [
            SkyOpts { inst: CloudInst::Face, ..old },
            SkyOpts { inst: CloudInst::Cell, ..old },
            SkyOpts { inst: CloudInst::Cell, packed_constants: true, ..old },
            SkyOpts { indexed: false, ..NEW },
            SkyOpts { packed_constants: false, ..NEW },
            NEW,
        ];
        let blend = device.features().contains(wgpu::Features::FLOAT32_BLENDABLE);
        for oit in [false, true] {
            if oit && rc_testdata::need(blend.then_some(()), "float target blending (FLOAT32_BLENDABLE)").is_none() {
                return;
            }
            let want = render_clouds(&device, &queue, old, oit);
            for (n, img) in want.iter().enumerate() {
                // Background cleared to (51, 102, 230): a cloud clearly raises the red.
                let covered = img.chunks(4).filter(|p| p[0] > 60).count();
                assert!(covered > 200, "view {n} (transparency {oit}): {covered} cloud pixels");
            }
            assert!(want[0] != want[1], "different views, different images");
            for opts in variants {
                let got = render_clouds(&device, &queue, opts, oit);
                for (n, (g, w)) in got.iter().zip(&want).enumerate() {
                    let diff = g.iter().zip(w).filter(|(a, b)| a != b).count();
                    assert!(diff == 0, "{opts:?} view {n} (transparency {oit}): {diff} bytes differ");
                }
            }
            let reversed = render_clouds_with(
                &device,
                &queue,
                NEW,
                oit.then(crate::oit::Options::from_switches),
                [2, 1, 0, 0, 3, 2],
            );
            let diff: usize =
                reversed.iter().zip(&want).map(|(g, w)| g.iter().zip(w).filter(|(a, b)| a != b).count()).sum();
            eprintln!("triangles wound the other way (transparency {oit}): {diff} bytes differ");
            assert!(diff > 0, "reversed triangles not seen (transparency {oit})");
        }
    }

    /// Real clouds through improved transparency in each form of its shaders (compact bounds,
    /// unrolled wavelet, projection written once): the images of the form before (RGBA32F bounds,
    /// wavelet loops, projection written on every frame) to the byte.
    #[test]
    fn cloud_transparency_forms_render_identically() {
        use crate::oit::Options;
        let Some((device, queue)) = gpu() else { return };
        let blend = device.features().contains(wgpu::Features::FLOAT32_BLENDABLE);
        if rc_testdata::need(blend.then_some(()), "float target blending (FLOAT32_BLENDABLE)").is_none() {
            return;
        }
        let render = |options| render_clouds_with(&device, &queue, NEW, Some(options), sky_mesh::QUAD_TRIANGLES);
        let want = render(Options::OLD);
        let forms = [
            Options { compact_bounds: true, ..Options::OLD },
            Options { unrolled_wavelet: true, ..Options::OLD },
            Options { projection_on_change: true, ..Options::OLD },
            Options::NEW,
        ];
        for options in forms {
            for (n, (got, want)) in render(options).iter().zip(&want).enumerate() {
                let diff = got.iter().zip(want).filter(|(a, b)| a != b).count();
                assert!(diff == 0, "{options:?} view {n}: {diff} bytes differ");
            }
        }
    }
}
