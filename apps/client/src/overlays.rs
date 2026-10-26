//! Fullscreen overlays (`docs/specs/hand-camera.md` § 1.2, `docs/specs/hud.md` § 5.5):
//! screen effects drawn after the hand with its projection (block interior, water, fire), and
//! interface camera overlays right after the vignette (equipped item, powder snow,
//! portal, nausea). A single vertex buffer in clip coordinates, two pipelines
//! (translucent, additive), "misc" textures in an array, block textures shared with
//! the terrain (current animation frames).

use hashbrown::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};

/// Side of the "misc" textures (all brought to this size).
const MISC: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 4],
    uv: [f32; 2],
    color: [f32; 4],
    tex: [u32; 2],
}

/// Overlay of the interface, in the game's order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GuiOverlay<'a> {
    /// Fullscreen `textures/<id>.png` texture (e.g. `minecraft:misc/pumpkinblur`), white
    /// with opacity `alpha`.
    Texture(&'a str, f32),
    /// Nausea (effects scale < 1): strength.
    Nausea(f32),
    /// Nether portal: portal block texture, opacity.
    Portal(f32),
    /// Spyglass: square mask (side min(W, H) × scale) centered, black bands around.
    Scope(f32),
}

/// Screen effects and overlays of a frame.
#[derive(Default)]
pub struct Frame<'a> {
    /// Hand field of view (degrees): projection of the screen effects.
    pub hud_fov: f32,
    /// Block that smothers the view: id of its state ("particle" texture of its model).
    pub block: Option<usize>,
    /// Underwater: RGBA color and texture offsets (yaw, pitch).
    pub water: Option<([f32; 4], f32, f32)>,
    /// On fire (first person).
    pub fire: bool,
    pub gui: Vec<GuiOverlay<'a>>,
}

pub struct Overlays {
    pipelines: [wgpu::RenderPipeline; 2],
    bind: wgpu::BindGroup,
    vb: Option<(wgpu::Buffer, u64)>,
    /// Vertex ranges: screen effects, then interface overlays (pipeline, range).
    screen: std::ops::Range<u32>,
    gui: Vec<(usize, std::ops::Range<u32>)>,
    /// Layer and smoothing of the "misc" textures.
    misc: HashMap<String, (u32, bool)>,
    models: Arc<rc_assets::BlockModels>,
    /// "Particle" texture per state, resolved on first request.
    particle: HashMap<usize, Option<u32>>,
    fire: Option<u32>,
    portal: Option<u32>,
}

/// "Misc" textures loaded at startup: screen effects, overlays, and the overlays of the
/// game's equippable items.
fn misc_ids(data: &rc_data::VersionData) -> Vec<String> {
    let mut ids: Vec<String> = [
        "minecraft:misc/underwater",
        "minecraft:misc/powder_snow_outline",
        "minecraft:misc/nausea",
        "minecraft:misc/spyglass_scope",
    ]
    .map(str::to_owned)
    .into();
    for i in &data.items {
        if !i.equip_overlay.is_empty() && !ids.contains(&i.equip_overlay) {
            ids.push(i.equip_overlay.clone());
        }
    }
    ids
}

fn norm(id: &str) -> String {
    let (ns, path) = rc_assets::source::split_id(id);
    format!("{ns}:{path}")
}

impl Overlays {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        blocks: (&wgpu::TextureView, &wgpu::TextureView),
        assets: Arc<rc_assets::Assets>,
        models: Arc<rc_assets::BlockModels>,
        data: Arc<rc_data::VersionData>,
    ) -> Self {
        // "Misc" textures brought to 256 × 256 nearest (the repeated water pattern stays
        // identical); `.mcmeta` `blur`: smoothed sampling.
        let ids = misc_ids(&data);
        let mut misc = HashMap::new();
        let mut pixels = Vec::new();
        for id in &ids {
            let Some(img) = rc_assets::texture::load(&assets, id) else { continue };
            let (ns, path) = rc_assets::source::split_id(id);
            let blur = assets
                .read(&format!("assets/{ns}/textures/{path}.png.mcmeta"))
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v.get("texture")?.get("blur")?.as_bool())
                .unwrap_or(false);
            misc.insert(norm(id), (misc.len() as u32, blur));
            pixels.extend_from_slice(&img.resized(MISC).rgba);
        }
        let layers = misc.len().max(1) as u32;
        pixels.resize((MISC * MISC * 4 * layers) as usize, 0);
        let misc_tex = wgpu::util::DeviceExt::create_texture_with_data(
            device,
            queue,
            &wgpu::TextureDescriptor {
                label: Some("overlays"),
                size: wgpu::Extent3d { width: MISC, height: MISC, depth_or_array_layers: layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        );
        let misc_view = misc_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("overlays"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let (nearest, linear) = (sampler(wgpu::FilterMode::Nearest), sampler(wgpu::FilterMode::Linear));
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
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
            label: Some("overlays"),
            entries: &[tex_entry(0), tex_entry(1), tex_entry(2), sampler_entry(3), sampler_entry(4)],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlays"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&misc_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(blocks.0) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(blocks.1) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&nearest) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&linear) },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlays"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlays.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        // Translucent (src·α + dst·(1−α)) and additive (src + dst), no depth.
        let additive = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let blends = [wgpu::BlendState::ALPHA_BLENDING, wgpu::BlendState { color: additive, alpha: additive }];
        let pipelines = blends.map(|blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("overlays"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Float32x4, 3 => Uint32x2],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })],
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
            })
        });
        let tex_of = |name: &str| models.texture_names.iter().position(|n| n == name).map(|i| i as u32);
        let (fire, portal) = (tex_of("minecraft:block/fire_1"), tex_of("minecraft:block/nether_portal"));
        Overlays {
            pipelines,
            bind,
            vb: None,
            screen: 0..0,
            gui: Vec::new(),
            misc,
            models,
            particle: HashMap::new(),
            fire,
            portal,
        }
    }

    /// "Particle" texture of a state's model (that of the break particles), among the
    /// block textures.
    fn particle_texture(&mut self, state: usize) -> Option<u32> {
        if let Some(&t) = self.particle.get(&state) {
            return t;
        }
        let name = self.models.particle(u32::try_from(state).ok()?);
        let tex = name.and_then(|n| self.models.texture_names.iter().position(|x| x == n).map(|i| i as u32));
        self.particle.insert(state, tex);
        tex
    }

    /// Builds and sends the frame's vertices. `size` = window in pixels, `scale` = scale
    /// of the interface, `layer` = current layer of a block texture (animations included).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        f: &Frame<'_>,
        size: (u32, u32),
        scale: u32,
        layer: impl Fn(u32) -> Option<u32>,
    ) {
        let mut v: Vec<Vertex> = Vec::new();
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let block_src = |t: u32| layer(t).map(|l| if l & 0x8000 != 0 { [3, l & 0x7fff] } else { [2, l] });
        // Screen effects: hand view space (empty pose), z = −0.5.
        let proj = crate::render::perspective(f.hud_fov, w / h, 0.05, 1000.0);
        let mut quad3 = |pose: Mat4, x: [f32; 2], y: [f32; 2], uv: [[f32; 2]; 2], color: [f32; 4], tex: [u32; 2]| {
            // Corners (x0,y0) (x1,y0) (x1,y1) (x0,y1) with the UVs (u0,v0) (u1,v0) (u1,v1) (u0,v1).
            let corners = [
                (x[0], y[0], uv[0][0], uv[0][1]),
                (x[1], y[0], uv[1][0], uv[0][1]),
                (x[1], y[1], uv[1][0], uv[1][1]),
                (x[0], y[1], uv[0][0], uv[1][1]),
            ];
            let p: Vec<Vertex> = corners
                .iter()
                .map(|&(x, y, u, vv)| Vertex {
                    pos: (proj * pose * Vec4::new(x, y, -0.5, 1.0)).to_array(),
                    uv: [u, vv],
                    color,
                    tex,
                })
                .collect();
            for i in [0, 1, 2, 2, 3, 0] {
                v.push(p[i]);
            }
        };
        if let Some(b) = f.block
            && let Some(src) = self.particle_texture(b).and_then(block_src)
        {
            // Block texture darkened (0x191919), horizontal mirror like the game.
            let c = 0x19 as f32 / 255.0;
            quad3(Mat4::IDENTITY, [-1.0, 1.0], [-1.0, 1.0], [[1.0, 1.0], [0.0, 0.0]], [c, c, c, 1.0], src);
        }
        if let Some((color, u, vo)) = f.water
            && let Some(&(l, _)) = self.misc.get("minecraft:misc/underwater")
        {
            // Four repeats, scrolling according to the rotation.
            quad3(Mat4::IDENTITY, [-1.0, 1.0], [-1.0, 1.0], [[u + 4.0, vo + 4.0], [u, vo]], color, [0, l]);
        }
        if f.fire
            && let Some(src) = self.fire.and_then(block_src)
        {
            let c = [1.0, 1.0, 1.0, 0xe5 as f32 / 255.0];
            for side in [1.0f32, -1.0] {
                let pose = Mat4::from_translation(Vec3::new(0.24 * side, -0.3, 0.0))
                    * Mat4::from_rotation_y(-side * std::f32::consts::PI / 18.0);
                quad3(pose, [-0.5, 0.5], [-0.5, 0.5], [[1.0, 1.0], [0.0, 0.0]], c, src);
            }
        }
        self.screen = 0..v.len() as u32;
        // Interface: rectangle (0, 0, W, H) in GUI units, projection 0..window/scale.
        let s = scale.max(1) as f32;
        let (gw, gh) = ((w / s).ceil(), (h / s).ceil());
        let to_clip = |x: f32, y: f32| [x * s / w * 2.0 - 1.0, 1.0 - y * s / h * 2.0, 0.0, 1.0];
        let mut ranges: Vec<(usize, std::ops::Range<u32>)> = Vec::new();
        let mut push_gui = |v: &mut Vec<Vertex>, pipeline: usize, r: [f32; 4], color: [f32; 4], tex: [u32; 2]| {
            let start = v.len() as u32;
            let corners =
                [(r[0], r[1], 0.0, 0.0), (r[2], r[1], 1.0, 0.0), (r[2], r[3], 1.0, 1.0), (r[0], r[3], 0.0, 1.0)];
            for i in [0, 1, 2, 2, 3, 0] {
                let (x, y, u, vv) = corners[i];
                v.push(Vertex { pos: to_clip(x, y), uv: [u, vv], color, tex });
            }
            let end = v.len() as u32;
            match ranges.last_mut() {
                Some((p, range)) if *p == pipeline => range.end = end,
                _ => ranges.push((pipeline, start..end)),
            }
        };
        for o in &f.gui {
            match *o {
                GuiOverlay::Texture(id, alpha) => {
                    if let Some(&(l, blur)) = self.misc.get(&norm(id)) {
                        push_gui(&mut v, 0, [0.0, 0.0, gw, gh], [1.0, 1.0, 1.0, alpha], [u32::from(blur), l]);
                    }
                }
                GuiOverlay::Portal(alpha) => {
                    if let Some(src) = self.portal.and_then(block_src) {
                        push_gui(&mut v, 0, [0.0, 0.0, gw, gh], [1.0, 1.0, 1.0, alpha], src);
                    }
                }
                GuiOverlay::Nausea(strength) => {
                    let Some(&(l, blur)) = self.misc.get("minecraft:misc/nausea") else { continue };
                    // Scale lerp(strength, 2, 1) around the center, color (0.2; 0.4; 0.2) × strength.
                    let k = 2.0 + (1.0 - 2.0) * strength;
                    let (cx, cy) = (gw / 2.0, gh / 2.0);
                    let r = [cx - cx * k, cy - cy * k, cx + (gw - cx) * k, cy + (gh - cy) * k];
                    let c = [0.2 * strength, 0.4 * strength, 0.2 * strength, 1.0];
                    push_gui(&mut v, 1, r, c, [u32::from(blur), l]);
                }
                GuiOverlay::Scope(scale) => {
                    let Some(&(l, blur)) = self.misc.get("minecraft:misc/spyglass_scope") else { continue };
                    // Integer side floor(min(W, H) × scale), centered (the game's integer divisions).
                    let (iw, ih) = (gw as i32, gh as i32);
                    let side = iw.min(ih) as f32;
                    let size = (side * (iw as f32 / side).min(ih as f32 / side) * scale).floor() as i32;
                    let (left, top) = ((iw - size) / 2, (ih - size) / 2);
                    let (right, bottom) = (left + size, top + size);
                    let f = |x: i32| x as f32;
                    push_gui(&mut v, 0, [f(left), f(top), f(right), f(bottom)], [1.0; 4], [u32::from(blur), l]);
                    let black = [0.0, 0.0, 0.0, 1.0];
                    for r in [
                        [0.0, f(bottom), gw, gh],
                        [0.0, 0.0, gw, f(top)],
                        [0.0, f(top), f(left), f(bottom)],
                        [f(right), f(top), gw, f(bottom)],
                    ] {
                        if r[2] > r[0] && r[3] > r[1] {
                            push_gui(&mut v, 0, r, black, [4, 0]);
                        }
                    }
                }
            }
        }
        self.gui = ranges;
        if v.is_empty() {
            return;
        }
        let bytes = bytemuck::cast_slice::<Vertex, u8>(&v);
        match &self.vb {
            Some((b, cap)) if *cap >= bytes.len() as u64 => queue.write_buffer(b, 0, bytes),
            _ => {
                let cap = (bytes.len() as u64).next_power_of_two();
                let b = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("overlays"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                queue.write_buffer(&b, 0, bytes);
                self.vb = Some((b, cap));
            }
        }
    }

    /// Screen effects (after the hand).
    pub fn draw_screen(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.screen.is_empty() {
            return;
        }
        let Some((vb, _)) = &self.vb else { return };
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.set_pipeline(&self.pipelines[0]);
        pass.draw(self.screen.clone(), 0..1);
    }

    /// Camera overlays (after the vignette, under the crosshair).
    pub fn draw_gui(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.gui.is_empty() {
            return;
        }
        let Some((vb, _)) = &self.vb else { return };
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        for (p, r) in &self.gui {
            pass.set_pipeline(&self.pipelines[*p]);
            pass.draw(r.clone(), 0..1);
        }
    }
}
