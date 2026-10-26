//! Text displays (`docs/specs/entities-objects.md` § 13.8): component split to the line width,
//! aligned lines (left, right, center), solid background, drop shadow, opacity. One
//! instance per glyph, one matrix per layer (background, shadow, text); layout kept per
//! entity as long as the text, the width and the flags do not change (except obfuscated text).
//!
//! Game modes: "polygon offset" (depth tested and written, lightmap, fog)
//! or "see-through" (flag 2: no depth, color only), back faces culled.

use hashbrown::HashMap;

use glam::{Mat4, Vec3};

use crate::entities::Entities;
use crate::entity_render::{DisplaySub, TextDraw};
use crate::gui::Gui;
use crate::nametag::{Glyph, glyphs};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TextInstance {
    face: u32,
    light: u32,
    glyph: Glyph,
}

/// Light marked "see-through" for the shader (neither lightmap nor fog).
const SEE_THROUGH: u32 = 0x8000_0000;

/// Depths of the layers in font pixels (toward the front of the text): background, shadow, text
/// above a shadow.
const BACKGROUND_DEPTH: f32 = -0.01;
const SHADOW_FRONT: f32 = 0.03;

/// Kept layout of a display.
struct Cached {
    key: (Option<rc_proto::nbt::Nbt>, i32, i8),
    /// Shadow and text glyphs, already placed (lines, alignment), width and height.
    shadow: Vec<Glyph>,
    text: Vec<Glyph>,
    width: f32,
    height: f32,
    frame: u64,
}

pub struct TextDisplays {
    /// Polygon offset (depth tested and written), see-through (no depth).
    offset: wgpu::RenderPipeline,
    see_through: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind: Option<wgpu::BindGroup>,
    /// Glyph sheet generation and layer buffer bound.
    bound: (u32, u64),
    faces: Vec<[[f32; 4]; 4]>,
    offset_inst: Vec<TextInstance>,
    see_inst: Vec<TextInstance>,
    /// Held map names: polygon offset, hand camera.
    hand_inst: Vec<TextInstance>,
    face_buf: Option<(wgpu::Buffer, u64)>,
    inst_buf: Option<(wgpu::Buffer, u64)>,
    /// Instances sent: polygon offset, see-through, hand.
    counts: (u32, u32, u32),
    cache: HashMap<i32, Cached>,
    /// Layout of map decoration names, per text.
    labels: Vec<(rc_proto::nbt::Nbt, Cached)>,
    frame: u64,
    face_gen: u64,
    /// Improved transparency (set by the renderer): opaque polygon-offset glyphs stored at the
    /// head (`solid` instances), backgrounds and translucent text in the stages.
    pub oit: bool,
    solid: u32,
    camera_layout: wgpu::BindGroupLayout,
    oit_set: Option<crate::oit::Set>,
}

/// Separates the shadow from the text: `all` (with shadow) contains `main` (without shadow) in order; the
/// rest is the shadow.
fn split_shadow(all: Vec<Glyph>, main: &[Glyph]) -> Vec<Glyph> {
    let same = |a: &Glyph, b: &Glyph| bytemuck::bytes_of(a) == bytemuck::bytes_of(b);
    let mut k = 0;
    let mut shadow = Vec::new();
    for g in all {
        if k < main.len() && same(&g, &main[k]) {
            k += 1;
        } else {
            shadow.push(g);
        }
    }
    if k < main.len() { Vec::new() } else { shadow }
}

/// ARGB color as RGBA bytes of the glyphs.
fn rgba(argb: u32) -> u32 {
    let (a, r, g, b) = (argb >> 24, (argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff);
    r | g << 8 | b << 16 | a << 24
}

/// Glyph alpha multiplied by the text opacity (byte).
fn with_opacity(g: Glyph, opacity: u32) -> Glyph {
    let a = (g.color >> 24) * opacity / 255;
    Glyph { color: (g.color & 0x00ff_ffff) | a << 24, ..g }
}

/// Horizontal offset of a line of width `lw` in a block of width `w` (flags 8:
/// left, 16: right, otherwise centered).
fn align(flags: i8, w: i32, lw: i32) -> f32 {
    if flags & 8 != 0 {
        0.0
    } else if flags & 16 != 0 {
        (w - lw) as f32
    } else {
        w as f32 / 2.0 - lw as f32 / 2.0
    }
}

impl TextDisplays {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text displays"),
            source: wgpu::ShaderSource::Wgsl(include_str!("text_display.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text displays"),
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
            label: Some("text displays"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let make = |label, depth: wgpu::DepthStencilState| {
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
                // Back faces culled: a display is only read from the front.
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Polygon offset toward the camera (slope 1, constant 10), depth written.
        let offset = make(
            "text displays",
            wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                bias: crate::render::depth_bias(10, 1.0),
            },
        );
        let see_through = make(
            "text displays (see-through)",
            wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        TextDisplays {
            offset,
            see_through,
            layout,
            sampler,
            bind: None,
            bound: (u32::MAX, u64::MAX),
            faces: Vec::new(),
            offset_inst: Vec::new(),
            see_inst: Vec::new(),
            hand_inst: Vec::new(),
            face_buf: None,
            inst_buf: None,
            counts: (0, 0, 0),
            cache: HashMap::new(),
            labels: Vec::new(),
            frame: 0,
            face_gen: 0,
            oit: false,
            solid: 0,
            camera_layout: camera_layout.clone(),
            oit_set: None,
        }
    }

    /// Improved-transparency pipelines (once): back faces culled.
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit_set.is_some() {
            return;
        }
        let module = crate::oit::module(
            device,
            "text displays (transparency)",
            include_str!("text_display.wgsl"),
            include_str!("text_display_oit.wgsl"),
        );
        let groups = [Some(&self.camera_layout), Some(&self.layout)];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TextInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Uint32, 1 => Uint32, 2 => Float32x4, 3 => Float32,
                4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
            ],
        })];
        let spec = crate::oit::Spec::new("text displays (transparency)", &groups, &module, "vs", &buffers, "fs")
            .cull(true);
        self.oit_set = Some(layouts.set(device, &spec));
    }

    /// Improved transparency: opaque polygon-offset glyphs, with the solids.
    pub fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.offset, 0..self.solid.min(self.counts.0));
    }

    /// Improved transparency: rest of the polygon-offset mode, stage `f`.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let (Some(bind), Some((buf, _)), Some(set)) = (&self.bind, &self.inst_buf, &self.oit_set) else { return };
        let range = self.solid.min(self.counts.0)..self.counts.0;
        if range.is_empty() {
            return;
        }
        f.set(pass, set);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, range);
    }

    /// Frame instances. `labels`: map decoration names (`TextDraw::label`);
    /// `default_background`: default background color (flag 4), text-background opacity of the
    /// options.
    pub fn prepare(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        draws: &[TextDraw],
        labels: &[rc_proto::nbt::Nbt],
        ents: &Entities,
        default_background: u32,
    ) {
        self.frame += 1;
        self.faces.clear();
        self.offset_inst.clear();
        self.see_inst.clear();
        self.hand_inst.clear();
        for d in draws {
            if d.label != u32::MAX {
                let Some(text) = labels.get(d.label as usize) else { continue };
                self.prepare_label(gui, queue, d, text, default_background);
                continue;
            }
            let Some(e) = ents.map.get(&d.id) else { continue };
            let Some(DisplaySub::Text(t)) = e.display.as_deref().and_then(|ds| ds.render.as_ref()).map(|r| &r.sub)
            else {
                continue;
            };
            let same = |c: &Cached| c.key.0 == t.text && c.key.1 == t.line_width && c.key.2 == t.flags;
            if self.cache.get(&d.id).is_none_or(|c| !same(c)) {
                let key = (t.text.clone(), t.line_width, t.flags);
                let (c, obfuscated) = build(gui, queue, key, self.frame);
                if obfuscated {
                    self.cache.remove(&d.id);
                    self.emit(d, &c, default_background);
                    continue;
                }
                self.cache.insert(d.id, c);
            }
            let Some(c) = self.cache.remove(&d.id) else { continue };
            self.emit(d, &c, default_background);
            self.cache.insert(d.id, Cached { frame: self.frame, ..c });
        }
        let frame = self.frame;
        self.cache.retain(|_, c| c.frame == frame);
        self.labels.retain(|(_, c)| c.frame == frame);
    }

    /// Map decoration name: a line with no splitting or flags, layout kept
    /// per text (except obfuscated text).
    fn prepare_label(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        d: &TextDraw,
        text: &rc_proto::nbt::Nbt,
        default_background: u32,
    ) {
        let frame = self.frame;
        if let Some(i) = self.labels.iter().position(|(t, _)| t == text) {
            let (t, c) = self.labels.swap_remove(i);
            self.emit(d, &c, default_background);
            self.labels.push((t, Cached { frame, ..c }));
            return;
        }
        let (c, obfuscated) = build(gui, queue, (Some(text.clone()), i32::MAX, 0), frame);
        self.emit(d, &c, default_background);
        if !obfuscated {
            self.labels.push((text.clone(), c));
        }
    }

    /// Layers and instances of a display.
    fn emit(&mut self, d: &TextDraw, c: &Cached, default_background: u32) {
        let see = d.flags & 2 != 0;
        let light = if see { SEE_THROUGH } else { d.light };
        let base = if d.label != u32::MAX {
            crate::entity_render::map_label_matrix(d.m, c.width)
        } else {
            d.m * Mat4::from_translation(Vec3::new(1.0 - c.width / 2.0, -c.height, 0.0))
        };
        let background = if d.flags & 4 != 0 { default_background } else { d.background };
        let opacity = u32::from(d.opacity);
        let shadowed = d.flags & 1 != 0;
        let out = if see {
            &mut self.see_inst
        } else if d.hand {
            &mut self.hand_inst
        } else {
            &mut self.offset_inst
        };
        let face = |z: f32, faces: &mut Vec<[[f32; 4]; 4]>| {
            faces.push((base * Mat4::from_translation(Vec3::new(0.0, 0.0, z))).to_cols_array_2d());
            (faces.len() - 1) as u32
        };
        if background != 0 {
            let f = face(BACKGROUND_DEPTH, &mut self.faces);
            let glyph = Glyph {
                rect: [-1.0, c.width, -1.0, c.height],
                skew: 0.0,
                uv: [0.0; 4],
                color: rgba(background),
                mode: 0,
            };
            out.push(TextInstance { face: f, light, glyph });
        }
        if !c.shadow.is_empty() {
            let f = face(0.0, &mut self.faces);
            out.extend(c.shadow.iter().map(|&g| TextInstance { face: f, light, glyph: with_opacity(g, opacity) }));
        }
        let f = face(if shadowed { SHADOW_FRONT } else { 0.0 }, &mut self.faces);
        out.extend(c.text.iter().map(|&g| TextInstance { face: f, light, glyph: with_opacity(g, opacity) }));
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, gui: &Gui) {
        if self.oit {
            self.offset_inst.sort_unstable_by_key(|t| !t.glyph.solid());
            self.solid = self.offset_inst.iter().take_while(|t| t.glyph.solid()).count() as u32;
        }
        self.counts = (self.offset_inst.len() as u32, self.see_inst.len() as u32, self.hand_inst.len() as u32);
        if self.counts == (0, 0, 0) {
            return;
        }
        let mut all = std::mem::take(&mut self.offset_inst);
        all.extend_from_slice(&self.see_inst);
        all.extend_from_slice(&self.hand_inst);
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
        if grow(
            &mut self.face_buf,
            bytemuck::cast_slice(&self.faces),
            wgpu::BufferUsages::STORAGE,
            "text display layers",
        ) {
            self.face_gen += 1;
        }
        grow(&mut self.inst_buf, bytemuck::cast_slice(&all), wgpu::BufferUsages::VERTEX, "text displays");
        all.truncate(self.counts.0 as usize);
        self.offset_inst = all;
        if self.bind.is_none() || self.bound != (gui.glyph_gen, self.face_gen) {
            let view = gui.glyph_view();
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("text displays"),
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

    /// "Polygon offset" mode, with the entities (group 0 = camera already bound).
    pub fn draw_offset(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.offset, 0..self.counts.0);
    }

    /// "See-through" mode, after the translucent terrain.
    pub fn draw_see_through(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.see_through, self.counts.0..self.counts.0 + self.counts.1);
    }

    /// Names of the held map, after the hand (group 0 = hand camera already bound).
    pub fn draw_hand(&self, pass: &mut wgpu::RenderPass<'_>) {
        let start = self.counts.0 + self.counts.1;
        self.draw(pass, &self.offset, start..start + self.counts.2);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline, range: std::ops::Range<u32>) {
        let (Some(bind), Some((buf, _))) = (&self.bind, &self.inst_buf) else { return };
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, range);
    }
}

/// Layout: split to the line width (empty text: no line), lines of
/// 9 + 1 pixels, alignment, shadow separated from the text. True second if the text is obfuscated.
fn build(gui: &mut Gui, queue: &wgpu::Queue, key: (Option<rc_proto::nbt::Nbt>, i32, i8), frame: u64) -> (Cached, bool) {
    let (nbt, line_width, flags) = &key;
    let chars = nbt
        .as_ref()
        .and_then(rc_text::from_nbt)
        .map(|c| rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang)))
        .unwrap_or_default();
    let lines = if chars.is_empty() { Vec::new() } else { rc_text::wrap(&*gui, &chars, *line_width as f32) };
    let widths: Vec<i32> = lines.iter().map(|l| rc_text::width(&*gui, l)).collect();
    let width = widths.iter().copied().max().unwrap_or(0);
    let height = lines.len() as f32 * 10.0 - 1.0;
    let obfuscated = chars.iter().any(|c| c.style.obfuscated);
    let (mut shadow, mut text) = (Vec::new(), Vec::new());
    for (i, (line, &lw)) in lines.iter().zip(&widths).enumerate() {
        if line.is_empty() {
            continue;
        }
        let dx = align(*flags, width, lw);
        let dy = i as f32 * 10.0;
        let place = |mut g: Glyph| {
            g.rect[0] += dx;
            g.rect[1] += dx;
            g.rect[2] += dy;
            g.rect[3] += dy;
            g
        };
        let main = glyphs(&gui.text_quads_left(queue, line, 0xffff_ffff, false));
        if flags & 1 != 0 {
            let all = glyphs(&gui.text_quads_left(queue, line, 0xffff_ffff, true));
            shadow.extend(split_shadow(all, &main).into_iter().map(place));
        }
        text.extend(main.into_iter().map(place));
    }
    (Cached { key, shadow, text, width: width as f32, height, frame }, obfuscated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(x: f32, color: u32) -> Glyph {
        Glyph { rect: [x, x + 5.0, 0.0, 8.0], skew: 0.0, uv: [0.0; 4], color, mode: 1 }
    }

    #[test]
    fn shadow_is_what_the_plain_pass_lacks() {
        let main = [g(0.0, 0xffff_ffff), g(6.0, 0xffff_ffff)];
        let all = vec![g(1.0, 0xff3f_3f3f), main[0], g(7.0, 0xff3f_3f3f), main[1]];
        let shadow = split_shadow(all, &main);
        assert_eq!(shadow.len(), 2);
        assert_eq!(shadow[0].rect[0], 1.0);
        assert_eq!(shadow[1].rect[0], 7.0);
    }

    #[test]
    fn shader_validates() {
        let m = wgpu::naga::front::wgsl::parse_str(include_str!("text_display.wgsl")).expect("parsing");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&m)
        .expect("valid shader");
    }

    #[test]
    fn alignment_flags() {
        assert_eq!(align(0, 40, 10), 15.0);
        assert_eq!(align(8, 40, 10), 0.0);
        assert_eq!(align(16, 40, 10), 30.0);
        // Half-pixel centering (odd widths).
        assert_eq!(align(0, 41, 10), 15.5);
    }

    #[test]
    fn opacity_scales_glyph_alpha_and_background_is_argb() {
        assert_eq!(with_opacity(g(0.0, 0xffff_ffff), 0x40).color, 0x40ff_ffff);
        assert_eq!(rgba(0x4000_0000), 0x4000_0000);
        assert_eq!(rgba(0x80ff_0000), 0x8000_00ff);
    }
}
