//! Sign text (`docs/specs/block-entities.md` § 3): lines laid out by the interface
//! (same glyphs and styles), placed in the frame of each face. One instance per glyph, one
//! matrix per face; the layout is kept per face as long as the sign's data does not
//! change.

use hashbrown::HashMap;

use glam::Mat4;

use crate::block_entities::{BlockEntities, Data, SignFace};
use crate::gui::Gui;
use crate::nametag::{Glyph, glyphs};

/// Sign face visible this frame (prepared by the block-entity renderer).
pub struct SignDraw {
    pub pos: [i32; 3],
    /// Matrices of the front and back faces (font pixels → camera frame).
    pub faces: [Mat4; 2],
    /// Block light (block × 16 | sky × 16 << 8).
    pub light: u32,
    pub hanging: bool,
    /// Distance² between the camera and the block center.
    pub d2: f64,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TextInstance {
    face: u32,
    light: u32,
    glyph: Glyph,
}

/// Text color per dye (§ 3.2).
pub(crate) const TEXT_COLORS: [u32; 16] = [
    0xFFFFFF, 0xFF681F, 0xFF00FF, 0x9AC0CD, 0xFFFF00, 0xBFFF00, 0xFF69B4, 0x808080, 0xD3D3D3, 0x00FFFF, 0xA020F0,
    0x0000FF, 0x8B4513, 0x00FF00, 0xFF0000, 0x000000,
];

/// Dark color: cream for glowing black, otherwise 0.4 × the color (per channel, integer).
pub(crate) fn dark_color(dye: u8, glowing: bool) -> u32 {
    if dye == 15 && glowing {
        return 0xF0EBCC;
    }
    let c = TEXT_COLORS[usize::from(dye & 15)];
    let ch = |s: u32| (((c >> s) & 0xff) as f64 * 0.4) as u32;
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

struct Cached {
    rev: u32,
    /// Text, outline (8 offset copies).
    text: Vec<Glyph>,
    outline: Vec<Glyph>,
    frame: u64,
}

pub struct BlockText {
    /// Text (polygon offset toward the camera) and outline (normal mode, drawn first).
    pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind: Option<wgpu::BindGroup>,
    /// Glyph sheet generation and face buffer bound.
    bound: (u32, u64),
    faces: Vec<[[f32; 4]; 4]>,
    inst: Vec<TextInstance>,
    outline: Vec<TextInstance>,
    face_buf: Option<(wgpu::Buffer, u64)>,
    inst_buf: Option<(wgpu::Buffer, u64)>,
    /// Instances sent: outline then text.
    counts: (u32, u32),
    cache: HashMap<([i32; 3], u8), Cached>,
    frame: u64,
    face_gen: u64,
}

impl BlockText {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sign text"),
            source: wgpu::ShaderSource::Wgsl(include_str!("block_text.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sign text"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sign text"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let make = |label, bias| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<TextInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Uint32, 1 => Uint32, 2 => Float32x4, 3 => Float32,
                            4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(crate::render::DEPTH_NEARER),
                    stencil: Default::default(),
                    bias,
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Polygon offset toward the camera (slope 1, constant 10) for the text only: in
        // the game, the glowing outline is drawn in normal mode, first, then the text in
        // polygon offset mode.
        let bias = crate::render::depth_bias(10, 1.0);
        let pipeline = make("sign text", bias);
        let outline_pipeline = make("sign text outline", wgpu::DepthBiasState::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        BlockText {
            pipeline,
            outline_pipeline,
            layout,
            sampler,
            bind: None,
            bound: (u32::MAX, u64::MAX),
            faces: Vec::new(),
            inst: Vec::new(),
            outline: Vec::new(),
            face_buf: None,
            inst_buf: None,
            counts: (0, 0),
            cache: HashMap::new(),
            frame: 0,
            face_gen: 0,
        }
    }

    /// Frame instances: visible faces, text (and outline of the glowing text).
    pub fn prepare(&mut self, gui: &mut Gui, queue: &wgpu::Queue, draws: &[SignDraw], bes: &BlockEntities) {
        self.frame += 1;
        self.faces.clear();
        self.inst.clear();
        self.outline.clear();
        for d in draws {
            let Some(e) = bes.get(d.pos) else { continue };
            let Data::Sign(sides) = &e.data else { continue };
            for (side, face) in sides.iter().enumerate() {
                if face.messages.iter().all(Option::is_none) {
                    continue;
                }
                let key = (d.pos, side as u8);
                let fresh = self.cache.get(&key).is_none_or(|c| c.rev != e.rev);
                if fresh {
                    let (text, outline) = build(gui, queue, face, d.hanging);
                    self.cache.insert(key, Cached { rev: e.rev, text, outline, frame: 0 });
                }
                let Some(c) = self.cache.get_mut(&key) else { continue };
                c.frame = self.frame;
                if c.text.is_empty() {
                    continue;
                }
                let index = self.faces.len() as u32;
                self.faces.push(d.faces[side].to_cols_array_2d());
                let light = if face.glowing { 0xf0f0 } else { d.light };
                // Outline: black, or camera within 16 blocks.
                if face.glowing && (face.color == 15 || d.d2 < 256.0) {
                    self.outline.extend(c.outline.iter().map(|&glyph| TextInstance { face: index, light, glyph }));
                }
                self.inst.extend(c.text.iter().map(|&glyph| TextInstance { face: index, light, glyph }));
            }
        }
        let frame = self.frame;
        self.cache.retain(|_, c| c.frame == frame);
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, gui: &Gui) {
        self.counts = (self.outline.len() as u32, self.inst.len() as u32);
        if self.counts.1 == 0 {
            return;
        }
        // Outline at the head of the buffer, text after.
        let mut all = std::mem::take(&mut self.outline);
        all.extend_from_slice(&self.inst);
        let grow = |buf: &mut Option<(wgpu::Buffer, u64)>, bytes: &[u8], usage: wgpu::BufferUsages, label| {
            let fresh = buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64);
            if fresh {
                let cap = (bytes.len() as u64).next_power_of_two().max(1024);
                *buf = Some((
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size: cap,
                        usage: usage | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    cap,
                ));
            }
            queue.write_buffer(&buf.as_ref().expect("created above").0, 0, bytes);
            fresh
        };
        if grow(&mut self.face_buf, bytemuck::cast_slice(&self.faces), wgpu::BufferUsages::STORAGE, "sign faces")
        {
            self.face_gen += 1;
        }
        grow(&mut self.inst_buf, bytemuck::cast_slice(&all), wgpu::BufferUsages::VERTEX, "sign text");
        all.clear();
        self.outline = all;
        if self.bind.is_none() || self.bound != (gui.glyph_gen, self.face_gen) {
            let view = gui.glyph_view();
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sign text"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.face_buf.as_ref().expect("created above").0.as_entire_binding(),
                    },
                ],
            }));
            self.bound = (gui.glyph_gen, self.face_gen);
        }
    }

    /// Drawing with the entities (group 0 = camera already bound).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(bind), Some((buf, _))) = (&self.bind, &self.inst_buf) else { return };
        let (outline, text) = self.counts;
        if text == 0 {
            return;
        }
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        if outline > 0 {
            pass.set_pipeline(&self.outline_pipeline);
            pass.draw(0..6, 0..outline);
        }
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..6, outline..outline + text);
    }
}

/// Layout of a face (§ 3.2): first line of the split of each message, centered,
/// line `i` at `y = i·h − 2h`; outline = 8 unstyled copies, offset by one pixel.
fn build(gui: &mut Gui, queue: &wgpu::Queue, face: &SignFace, hanging: bool) -> (Vec<Glyph>, Vec<Glyph>) {
    let (max, h) = if hanging { (60.0, 9.0) } else { (90.0, 10.0) };
    let base = TEXT_COLORS[usize::from(face.color & 15)];
    let dark = dark_color(face.color, face.glowing);
    let color = 0xff00_0000 | if face.glowing { base } else { dark };
    let (mut text, mut outline) = (Vec::new(), Vec::new());
    for (i, m) in face.messages.iter().enumerate() {
        let Some(n) = m else { continue };
        let Some(comp) = rc_text::from_nbt(n) else { continue };
        let chars = rc_text::styled_chars(&rc_text::flatten(&comp, &gui.lang));
        let Some(line) = rc_text::wrap(&*gui, &chars, max).into_iter().next() else { continue };
        if line.is_empty() {
            continue;
        }
        let w = rc_text::width(&*gui, &line);
        // Centering with integer division (the template centers at half-pixel).
        let dx = (-(w / 2)) as f32 + w as f32 / 2.0;
        let dy = i as f32 * h - 2.0 * h;
        let shift = |g: &mut Glyph, ox: f32, oy: f32| {
            g.rect[0] += dx + ox;
            g.rect[1] += dx + ox;
            g.rect[2] += dy + oy;
            g.rect[3] += dy + oy;
        };
        for mut g in glyphs(&gui.text_quads(queue, &line, color, 0)) {
            shift(&mut g, 0.0, 0.0);
            text.push(g);
        }
        if face.glowing {
            let plain: Vec<rc_text::StyledChar> = line
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    c.style.color = None;
                    c.style.underlined = false;
                    c.style.strikethrough = false;
                    c
                })
                .collect();
            let quads = glyphs(&gui.text_quads(queue, &plain, 0xff00_0000 | dark, 0));
            for (ox, oy) in
                [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)]
            {
                for &q in &quads {
                    let mut g = q;
                    shift(&mut g, ox, oy);
                    outline.push(g);
                }
            }
        }
    }
    (text, outline)
}
