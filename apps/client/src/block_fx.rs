//! End beacon and gateway beams, portal and gateway effect
//! (`docs/specs/block-entities.md` § 4.9 to 4.11): geometry prepared by the block-entity
//! renderer, dedicated pipelines (opaque core, translucent halo, portal with coordinates
//! projected onto the screen).

use rc_assets::texture::Image;

use crate::render::upload::Uploader;
use crate::render::upload::per_frame::FrameBuffer;
use crate::small_layers::READ_BACK;

/// Beam vertex: position (camera frame), repeated UVs, RGBA color, texture
/// (0 beacon, 1 gateway).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BeamVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub color: u32,
    pub tex: u32,
}

/// Portal vertex: position and number of layers (15 portal, 16 gateway).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PortalVertex {
    pub pos: [f32; 3],
    pub layers: u32,
}

/// Frame geometry: cores, halos, portals.
#[derive(Default)]
pub struct FxFrame {
    pub beams: Vec<BeamVertex>,
    pub halos: Vec<BeamVertex>,
    pub portals: Vec<PortalVertex>,
    /// Portal time.
    pub time: f32,
}

impl FxFrame {
    pub fn clear(&mut self) {
        self.beams.clear();
        self.halos.clear();
        self.portals.clear();
    }
}

/// Bytes of the parameter uniform (portal time).
const PARAMS_BYTES: u64 = 16;

pub struct BlockFx {
    beam: wgpu::RenderPipeline,
    halo: wgpu::RenderPipeline,
    portal: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: FrameBuffer,
    bind: Option<wgpu::BindGroup>,
    /// Cores then halos, in one buffer.
    beam_buffer: FrameBuffer,
    portal_buffer: FrameBuffer,
    /// Vertices sent: cores, halos, portals.
    counts: (u32, u32, u32),
    /// Camera layout and halos in improved transparency (made on demand).
    camera_layout: wgpu::BindGroupLayout,
    oit: Option<crate::oit::Set>,
}

fn texture(device: &wgpu::Device, queue: &wgpu::Queue, img: &Image) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: img.width.max(1), height: img.height.max(1), depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("block effects"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let white = [255u8; 4];
    let data: &[u8] = if img.rgba.len() >= (size.width * size.height * 4) as usize { &img.rgba } else { &white };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: Default::default(),
        },
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width * 4), rows_per_image: None },
        size,
    );
    tex.create_view(&Default::default())
}

impl BlockFx {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("block effects"),
            source: wgpu::ShaderSource::Wgsl(include_str!("block_fx.wgsl").into()),
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("block effects"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("block effects"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let beam_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BeamVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4, 3 => Uint32],
        };
        let portal_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PortalVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32],
        };
        let make = |label, vs: &str, fs: &str, buf: &wgpu::VertexBufferLayout<'_>, blend, write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(buf.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                // Back faces culled (the game's default setting for the three types): without
                // this, the halo's back faces add up and double it.
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(crate::render::DEPTH_NEARER),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let beam = make("beams", "vs_beam", "fs_beam", &beam_layout, None, true);
        let halo = make(
            "beam halos",
            "vs_beam",
            "fs_halo",
            &beam_layout,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
        );
        let portal = make("End portals", "vs_portal", "fs_portal", &portal_layout, None, true);
        let mut params = FrameBuffer::new("block effects", wgpu::BufferUsages::UNIFORM | READ_BACK, PARAMS_BYTES);
        // Made now: the bind group of `set_textures` reads it before the first write.
        params.reserve(device, PARAMS_BYTES);
        BlockFx {
            beam,
            halo,
            portal,
            layout,
            params,
            bind: None,
            beam_buffer: FrameBuffer::new("beams", wgpu::BufferUsages::VERTEX | READ_BACK, 1024),
            portal_buffer: FrameBuffer::new("End portals", wgpu::BufferUsages::VERTEX | READ_BACK, 1024),
            counts: (0, 0, 0),
            camera_layout: camera_layout.clone(),
            oit: None,
        }
    }

    /// Halo pipelines in improved transparency (once): back faces culled.
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit.is_some() {
            return;
        }
        let module = layouts.module(
            device,
            "beam halos (transparency)",
            include_str!("block_fx.wgsl"),
            include_str!("block_fx_oit.wgsl"),
        );
        let groups = [Some(&self.camera_layout), Some(&self.layout)];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BeamVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4, 3 => Uint32],
        })];
        let spec =
            crate::oit::Spec::new("beam halos (transparency)", &groups, &module, "vs_beam", &buffers, "fs_halo")
                .cull(true);
        self.oit = Some(layouts.set(device, &spec));
    }

    /// Improved transparency: cores and portals (opaque), with the solids.
    pub fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(bind) = &self.bind else { return };
        let (beams, _, portals) = self.counts;
        pass.set_bind_group(1, bind, &[]);
        if portals > 0
            && let Some(buf) = self.portal_buffer.buffer()
        {
            pass.set_pipeline(&self.portal);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..portals, 0..1);
        }
        if let Some(buf) = self.beam_buffer.buffer().filter(|_| beams > 0) {
            pass.set_pipeline(&self.beam);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..beams, 0..1);
        }
    }

    /// Improved transparency: halos, stage `f`.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let (Some(bind), Some(set)) = (&self.bind, &self.oit) else { return };
        let (beams, halos, _) = self.counts;
        if let Some(buf) = self.beam_buffer.buffer().filter(|_| halos > 0) {
            f.set(pass, set);
            pass.set_bind_group(1, bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(beams..beams + halos, 0..1);
        }
    }

    /// Textures (read once): beacon beam, gateway beam, End sky, portal.
    pub fn set_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, imgs: [&Image; 4]) {
        let views = imgs.map(|i| texture(device, queue, i));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("block effects"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("block effects"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&views[0]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[1]) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&views[2]) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&views[3]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.params.buffer().expect("uniform made at creation").as_entire_binding(),
                },
            ],
        }));
    }

    /// Sends the frame's geometry through the uploader (its transit ring under
    /// `RUSTCRAFT_OPT_FRAME_WRITES_RING`, instead of a staging buffer made and dropped by the queue
    /// per write). Cores and halos are copied side by side straight into the transit: no joined
    /// copy of the two lists is built first.
    pub fn upload(&mut self, queue: &wgpu::Queue, up: &mut Uploader, f: &FxFrame) {
        self.counts = (f.beams.len() as u32, f.halos.len() as u32, f.portals.len() as u32);
        if self.counts == (0, 0, 0) {
            return;
        }
        self.params.write(up, queue, bytemuck::cast_slice(&[f.time, 0.0, 0.0, 0.0]));
        let (cores, halos): (&[u8], &[u8]) = (bytemuck::cast_slice(&f.beams), bytemuck::cast_slice(&f.halos));
        self.beam_buffer.write_with(up, queue, (cores.len() + halos.len()) as u64, |mut transit| {
            transit.slice(..cores.len()).copy_from_slice(cores);
            transit.slice(cores.len()..).copy_from_slice(halos);
        });
        self.portal_buffer.write(up, queue, bytemuck::cast_slice(&f.portals));
    }

    /// Cores and portals (opaque), then halos (group 0 = camera already bound).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(bind) = &self.bind else { return };
        let (beams, halos, portals) = self.counts;
        if beams + halos + portals == 0 {
            return;
        }
        pass.set_bind_group(1, bind, &[]);
        if portals > 0
            && let Some(buf) = self.portal_buffer.buffer()
        {
            pass.set_pipeline(&self.portal);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..portals, 0..1);
        }
        if let Some(buf) = self.beam_buffer.buffer().filter(|_| beams + halos > 0) {
            pass.set_vertex_buffer(0, buf.slice(..));
            if beams > 0 {
                pass.set_pipeline(&self.beam);
                pass.draw(0..beams, 0..1);
            }
            if halos > 0 {
                pass.set_pipeline(&self.halo);
                pass.draw(beams..beams + halos, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A beam vertex whose fields tell frame `n`, list `list` and index `i` apart.
    fn numbered_beam(n: usize, list: u32, i: usize) -> BeamVertex {
        let v = (n * 10_000 + i) as f32;
        BeamVertex { pos: [v, -v, list as f32], uv: [v * 0.5, v + 1.0], color: (n as u32) << 24 | i as u32, tex: list }
    }

    fn numbered_frame(n: usize, (cores, halos, portals): (usize, usize, usize)) -> FxFrame {
        FxFrame {
            beams: (0..cores).map(|i| numbered_beam(n, 0, i)).collect(),
            halos: (0..halos).map(|i| numbered_beam(n, 1, i)).collect(),
            portals: (0..portals).map(|i| PortalVertex { pos: [i as f32, n as f32, -1.0], layers: 15 }).collect(),
            time: n as f32 * 0.25,
        }
    }

    /// The cores then the halos, the portals and the time reach the GPU the same byte for byte
    /// through the uploader, with and without its transit ring, as the writes of before left them
    /// (the cores and halos joined in one list first): over frames that grow the buffers, hold only
    /// cores or only halos, and shrink.
    #[test]
    fn beams_halos_and_portals_reach_the_gpu_unchanged() {
        use crate::render::upload::tests::{gpu, read};
        let Some((device, queue)) = gpu() else { return };
        let camera = crate::render::camera_layout(&device, false);
        let frames = [(6, 12, 6), (300, 0, 0), (0, 420, 36), (2000, 1500, 600), (6, 6, 0)];
        for ring in [false, true] {
            let mut fx = BlockFx::new(&device, &camera, wgpu::TextureFormat::Rgba8Unorm);
            let mut up = Uploader::new(&device, ring);
            for (n, &counts) in frames.iter().enumerate() {
                let frame = numbered_frame(n, counts);
                fx.upload(&queue, &mut up, &frame);
                up.submit(&queue, device.create_command_encoder(&Default::default()).finish());
                let mut joined = frame.beams.clone();
                joined.extend_from_slice(&frame.halos);
                let held = |buffer: &FrameBuffer, len: usize| {
                    buffer.buffer().map_or(Vec::new(), |b| read(&device, &queue, b)[..len].to_vec())
                };
                let (beams, portals): (&[u8], &[u8]) =
                    (bytemuck::cast_slice(&joined), bytemuck::cast_slice(&frame.portals));
                assert!(held(&fx.beam_buffer, beams.len()) == beams, "ring={ring} frame {n}: cores and halos");
                assert!(held(&fx.portal_buffer, portals.len()) == portals, "ring={ring} frame {n}: portals");
                let time = [frame.time, 0.0, 0.0, 0.0];
                let time: &[u8] = bytemuck::cast_slice(&time);
                assert!(held(&fx.params, time.len()) == time, "ring={ring} frame {n}: time");
                assert_eq!(fx.counts, (counts.0 as u32, counts.1 as u32, counts.2 as u32));
            }
        }
    }
}
