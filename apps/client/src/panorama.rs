//! Title screen panorama (`docs/specs/title-screen-servers.md` § 1.1): cube texture of the six
//! images `title/background/panorama_N`, vertical field of view 85°, 10° downward tilt,
//! rotation of 2°/s at the default speed, then `panorama_overlay` veil stretched over the screen.
//! Not yet wired into the normal flow: `RUSTCRAFT_PANORAMA=1` draws it over the world.

use std::time::Instant;

use glam::Mat4;
#[cfg(test)]
use glam::Vec3;
use rc_assets::texture::Image;

/// Source image of each layer of the cube, in the order +X, −X, +Y, −Y, +Z, −Z.
pub const LAYER_FACES: [usize; 6] = [1, 3, 5, 4, 0, 2];
const FOV_DEG: f32 = 85.0;
const TILT_DEG: f32 = 10.0;

/// Six faces (`panorama_0` … `panorama_5`) and veil.
pub struct PanoramaAssets {
    pub faces: Vec<Image>,
    pub overlay: Option<Image>,
}

impl PanoramaAssets {
    /// Reads the images (the asset index comes before the jar, whose faces are 1 × 1);
    /// `None` if a face is missing or if their sizes differ.
    pub fn load(assets: &rc_assets::Assets) -> Option<Self> {
        let read = |p: &str| assets.image(&format!("assets/minecraft/textures/gui/title/background/{p}.png"));
        let faces: Vec<Image> = (0..6).map(|i| read(&format!("panorama_{i}"))).collect::<Option<_>>()?;
        let (w, h) = (faces[0].width, faces[0].height);
        if faces.iter().any(|f| f.width != w || f.height != h) {
            eprintln!("panorama: faces of different sizes");
            return None;
        }
        let overlay = read("panorama_overlay");
        Some(PanoramaAssets { faces, overlay })
    }
}

/// Cube layers: faces reordered and flipped vertically, as at the game's copy.
pub fn cube_layers(faces: &[Image]) -> Vec<u8> {
    let mut out = Vec::with_capacity(faces.iter().map(|f| f.rgba.len()).sum());
    for &i in &LAYER_FACES {
        let f = &faces[i];
        let row = f.width as usize * 4;
        for y in (0..f.height as usize).rev() {
            out.extend_from_slice(&f.rgba[y * row..(y + 1) * row]);
        }
    }
    out
}

/// Rotation: `spin + Δt(ticks) × speed × 0.1`, brought back into [−180, 180).
pub fn advance(spin: f32, dt_ms: f32, speed: f32) -> f32 {
    let s = spin + dt_ms / 50.0 * speed * 0.1;
    (s + 180.0).rem_euclid(360.0) - 180.0
}

/// Projection × view: 180° then 10° around X, then −spin around Y.
pub fn view_proj(aspect: f32, spin: f32) -> Mat4 {
    let proj = crate::render::perspective(FOV_DEG, aspect, 0.05, 10.0);
    let view = Mat4::from_rotation_x(std::f32::consts::PI)
        * Mat4::from_rotation_x(TILT_DEG.to_radians())
        * Mat4::from_rotation_y((-spin).to_radians());
    proj * view
}

/// Look direction at the screen center, in the cube frame (for the tests).
#[cfg(test)]
pub fn center_dir(spin: f32) -> Vec3 {
    let view = Mat4::from_rotation_x(std::f32::consts::PI)
        * Mat4::from_rotation_x(TILT_DEG.to_radians())
        * Mat4::from_rotation_y((-spin).to_radians());
    // Camera looking at −Z (right-handed convention): world direction = inverse(view) × (0, 0, −1).
    view.inverse().transform_vector3(Vec3::NEG_Z)
}

pub struct Panorama {
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    cube_pipe: wgpu::RenderPipeline,
    overlay_pipe: Option<wgpu::RenderPipeline>,
    /// Current angle (degrees), global: survives screen changes.
    pub spin: f32,
    /// `panoramaScrollSpeed` option (0 to 1).
    pub speed: f32,
    last: Option<Instant>,
}

impl Panorama {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        assets: &PanoramaAssets,
    ) -> Self {
        use wgpu::util::DeviceExt;
        let (w, h) = (assets.faces[0].width, assets.faces[0].height);
        let cube = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("panorama"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 6 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &cube_layers(&assets.faces),
        );
        let cube_view = cube.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        let overlay_img = assets.overlay.clone().unwrap_or_else(|| Image { width: 1, height: 1, rgba: vec![0; 4] });
        let overlay = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("panorama veil"),
                size: wgpu::Extent3d { width: overlay_img.width, height: overlay_img.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &overlay_img.rgba,
        );
        let overlay_view = overlay.create_view(&Default::default());
        // Cube: linear filtering (the game's "blurry" texture); veil: nearest neighbor.
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("panorama"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("veil"), ..Default::default() });
        let tex_entry = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("panorama"),
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
                tex_entry(1, wgpu::TextureViewDimension::Cube),
                sampler_entry(2),
                tex_entry(3, wgpu::TextureViewDimension::D2),
                sampler_entry(4),
            ],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("panorama"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("panorama"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&cube_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&linear) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&overlay_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&nearest) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("panorama"),
            source: wgpu::ShaderSource::Wgsl(include_str!("panorama.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("panorama"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let make = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                // World pass (depth buffer present): neither test nor write.
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
            })
        };
        let cube_pipe = make("panorama", "vs_cube", "fs_cube", None);
        let overlay_pipe = assets
            .overlay
            .is_some()
            .then(|| make("panorama veil", "vs_overlay", "fs_overlay", Some(wgpu::BlendState::ALPHA_BLENDING)));
        Panorama { uniform, bind, cube_pipe, overlay_pipe, spin: 0.0, speed: 1.0, last: None }
    }

    /// Advances the rotation in real time and sends the matrix.
    pub fn prepare(&mut self, queue: &wgpu::Queue, size: (u32, u32)) {
        let now = Instant::now();
        if let Some(last) = self.last {
            self.spin = advance(self.spin, (now - last).as_secs_f32() * 1e3, self.speed);
        }
        self.last = Some(now);
        let m = view_proj(size.0 as f32 / size.1.max(1) as f32, self.spin);
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&m.to_cols_array()));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_pipeline(&self.cube_pipe);
        pass.draw(0..36, 0..1);
        if let Some(p) = &self.overlay_pipe {
            pass.set_pipeline(p);
            pass.draw(0..6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spin_speed_and_wrap() {
        // 1 s at speed 1: 2°.
        assert!((advance(0.0, 1000.0, 1.0) - 2.0).abs() < 1e-5);
        assert!((advance(179.0, 1000.0, 1.0) - (-179.0)).abs() < 1e-4);
        assert_eq!(advance(10.0, 1000.0, 0.0), 10.0);
    }

    #[test]
    fn layers_order_and_flip() {
        // Face i: 1 × 2, top row = i, bottom row = 10 + i.
        let faces: Vec<Image> =
            (0..6u8).map(|i| Image { width: 1, height: 2, rgba: vec![i, 0, 0, 0, 10 + i, 0, 0, 0] }).collect();
        let l = cube_layers(&faces);
        let firsts: Vec<u8> = l.chunks(8).map(|c| c[0]).collect();
        assert_eq!(firsts, [11, 13, 15, 14, 10, 12]);
    }

    #[test]
    fn view_looks_slightly_down() {
        // spin 0: the look direction goes toward +Z of the cube (180° rotation around X), 10° below the horizon
        // of the flipped cube, i.e. positive y component in the cube frame.
        let d = center_dir(0.0);
        assert!(d.z > 0.98, "{d}");
        assert!((d.y - TILT_DEG.to_radians().sin()).abs() < 1e-4, "{d}");
        // The rotation turns the look direction around Y.
        let d90 = center_dir(90.0);
        assert!(d90.x.abs() > 0.98, "{d90}");
    }
}
