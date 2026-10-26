//! End beacon and gateway beams, portal and gateway effect
//! (`docs/specs/block-entities.md` § 4.9 to 4.11): geometry prepared by the block-entity
//! renderer, dedicated pipelines (opaque core, translucent halo, portal with coordinates
//! projected onto the screen).

use rc_assets::texture::Image;

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

pub struct BlockFx {
    beam: wgpu::RenderPipeline,
    halo: wgpu::RenderPipeline,
    portal: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    bind: Option<wgpu::BindGroup>,
    beam_buf: Option<(wgpu::Buffer, u64)>,
    portal_buf: Option<(wgpu::Buffer, u64)>,
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
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("block effects"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        BlockFx {
            beam,
            halo,
            portal,
            layout,
            params,
            bind: None,
            beam_buf: None,
            portal_buf: None,
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
        let module = crate::oit::module(
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
            && let Some((buf, _)) = &self.portal_buf
        {
            pass.set_pipeline(&self.portal);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..portals, 0..1);
        }
        if let Some((buf, _)) = self.beam_buf.as_ref().filter(|_| beams > 0) {
            pass.set_pipeline(&self.beam);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..beams, 0..1);
        }
    }

    /// Improved transparency: halos, stage `f`.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let (Some(bind), Some(set)) = (&self.bind, &self.oit) else { return };
        let (beams, halos, _) = self.counts;
        if let Some((buf, _)) = self.beam_buf.as_ref().filter(|_| halos > 0) {
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
                wgpu::BindGroupEntry { binding: 5, resource: self.params.as_entire_binding() },
            ],
        }));
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, f: &FxFrame) {
        self.counts = (f.beams.len() as u32, f.halos.len() as u32, f.portals.len() as u32);
        if self.counts == (0, 0, 0) {
            return;
        }
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&[f.time, 0.0, 0.0, 0.0]));
        let grow = |buf: &mut Option<(wgpu::Buffer, u64)>, bytes: &[u8], label| {
            if bytes.is_empty() {
                return;
            }
            if buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64) {
                let cap = (bytes.len() as u64).next_power_of_two().max(1024);
                *buf = Some((
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size: cap,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    cap,
                ));
            }
            queue.write_buffer(&buf.as_ref().expect("created above").0, 0, bytes);
        };
        let mut beams = f.beams.clone();
        beams.extend_from_slice(&f.halos);
        grow(&mut self.beam_buf, bytemuck::cast_slice(&beams), "beams");
        grow(&mut self.portal_buf, bytemuck::cast_slice(&f.portals), "End portals");
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
            && let Some((buf, _)) = &self.portal_buf
        {
            pass.set_pipeline(&self.portal);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..portals, 0..1);
        }
        if let Some((buf, _)) = &self.beam_buf.as_ref().filter(|_| beams + halos > 0) {
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
