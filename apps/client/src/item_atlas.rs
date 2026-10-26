//! GUI item atlas (`docs/specs/items.md` part 2 § 5-8): each distinct item render
//! is drawn once in 3D into a cell of `16 · scale` pixels (orthographic projection,
//! reversed depth, item lighting), then the cell is copied into the interface.

use hashbrown::HashMap;

use rc_assets::item::{BakedItem, ItemModels, Layer, Mat4, apply, mat_mul, scale, translate};
use rc_assets::texture::Image;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
    /// 0: no glint; 1: enchantment glint (sprite UV); 2: special glint (decal's
    /// `guv` UV).
    glint: f32,
    /// Special glint coordinates ([`special_glint_uv`], interface scale).
    guv: [f32; 2],
}

/// GUI item lights already transformed (`items.md` part 2 § 6.1): flat items, then blocks.
const LIGHTS_FRONT: [[f32; 3]; 2] = [[-0.222_519, -0.171_499, 0.959_726], [-0.215_012, -0.971_825, 0.096_568]];
const LIGHTS_SIDE: [[f32; 3]; 2] = [[-0.933_439, -0.262_695, -0.244_300], [-0.103_571, -0.976_607, 0.188_446]];

struct Slot {
    cell: u32,
    used: bool,
    animated: bool,
    /// A layer carries the glint (`RUSTCRAFT_OPT_ANIMDIRTY`: the only reason to redraw on the clock).
    glint: bool,
    /// Animated sprites read by the cell (`RUSTCRAFT_OPT_ANIMDIRTY`).
    sprites: Box<[u32]>,
    /// Last draw (animated cells: redrawn at most every `ANIMATED_MS`).
    drawn: std::time::Instant,
    /// Texture tick at the last draw.
    drawn_tick: u64,
}

/// Minimum interval between two draws of an animated cell (glint, animated sprite): 4 ms,
/// under the perceptible threshold even at 240 Hz (the glint scrolls in 27.5 s).
const ANIMATED_MS: u128 = 4;

/// Atlas switches, read once at its creation (default: new path, `=0`: old).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasOpts {
    /// `RUSTCRAFT_OPT_ATLASBORROW`: baked model borrowed instead of cloned per layer and per
    /// draw; special quads, pass vertices and freed cells in kept buffers.
    pub borrow: bool,
    /// `RUSTCRAFT_OPT_GLINTCLOCK`: one shared 4 ms clock, all the animated cells
    /// redrawn in the same frame (one pass per 4 ms instead of one per desynchronized cell).
    pub glint_clock: bool,
    /// `RUSTCRAFT_OPT_ANIMDIRTY`: a cell with an animated sprite is redrawn only when one of its
    /// sprites changed in the sheet; only the glint follows the clock.
    pub anim_dirty: bool,
}

impl AtlasOpts {
    #[cfg(test)]
    pub const OLD: AtlasOpts = AtlasOpts { borrow: false, glint_clock: false, anim_dirty: false };
    #[cfg(test)]
    pub const ALL: AtlasOpts = AtlasOpts { borrow: true, glint_clock: true, anim_dirty: true };

    pub fn from_env() -> AtlasOpts {
        let on = |v: Option<&str>| v != Some("0");
        AtlasOpts {
            borrow: on(rc_data::env_str!("RUSTCRAFT_OPT_ATLASBORROW")),
            glint_clock: on(rc_data::env_str!("RUSTCRAFT_OPT_GLINTCLOCK")),
            anim_dirty: on(rc_data::env_str!("RUSTCRAFT_OPT_ANIMDIRTY")),
        }
    }
}

pub struct ItemAtlas {
    pub models: ItemModels,
    /// Rectangle (x, y, w, h) of each sprite in the sprite sheet, in fractions.
    rects: Vec<[f32; 4]>,
    sprite_bind: wgpu::BindGroup,
    solid: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    clear: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    started: std::time::Instant,
    /// `glintSpeed` and `glintStrength` settings (defaults 0.5 and 0.75).
    pub glint: (f32, f32),
    /// Target: color and depth, side in pixels, side of a cell.
    target: Option<(wgpu::Texture, wgpu::TextureView, wgpu::TextureView)>,
    pub size: u32,
    pub cell: u32,
    slots: HashMap<u64, Slot>,
    free: Vec<u32>,
    next: u32,
    /// Vertices to draw this frame: clears, solids, translucents.
    clears: Vec<Vertex>,
    solids: Vec<Vertex>,
    blended: Vec<(f32, [Vertex; 6])>,
    /// Reused vertex buffer (capacity in bytes).
    vb: Option<(wgpu::Buffer, u64)>,
    /// Special models (heads, banners…) and index of their first frame in `rects`.
    special: crate::item_special::SpecialAssets,
    special_base: usize,
    /// Sprite sheet (head skins written afterwards), side, first slot reserved
    /// for skins, slots taken and head identity → slot.
    sheet: wgpu::Texture,
    sheet_side: u32,
    head_base: usize,
    head_next: usize,
    head_slots: HashMap<u128, usize>,
    /// Animated sprites (sprite index, playback): rewritten into the sheet when they change.
    anims: Vec<(usize, crate::sprite_anim::SpriteAnim)>,
    /// The target was recreated: the GUI binding must be redone.
    pub recreated: bool,
    /// Number of the current target (changes on every recreation).
    pub generation: u32,
    opts: AtlasOpts,
    /// Shared clock: last beat and beat due this frame (`glint_clock`).
    last_clock: std::time::Instant,
    clock_due: bool,
    /// Texture ticks, animated sprite (per sprite index) and tick of its last change.
    tick_no: u64,
    anim_sprite: Vec<bool>,
    stamp: Vec<u64>,
    /// Kept buffers (`borrow`): pass vertices, quads of a special model.
    all: Vec<Vertex>,
    squads: Vec<crate::item_special::SQuad>,
    /// Forced instant (tests): duration since `started`.
    #[cfg(test)]
    fake_now: Option<std::time::Duration>,
}

#[cfg(test)]
mod tests;

/// 64 × 64 slots reserved in the sheet for the player head skins.
const HEAD_SLOTS: usize = 48;

/// 2D sprite sheet (successive rows), fractions of each rectangle.
fn pack(sprites: &[Image]) -> (Image, Vec<[f32; 4]>) {
    let area: u64 = sprites.iter().map(|s| u64::from(s.width) * u64::from(s.height)).sum();
    let widest = sprites.iter().map(|s| s.width).max().unwrap_or(16);
    let mut side = 256u32.max(widest.next_power_of_two());
    while u64::from(side) * u64::from(side) < area * 2 {
        side *= 2;
    }
    loop {
        let (mut x, mut y, mut row) = (0u32, 0u32, 0u32);
        let mut pos = Vec::with_capacity(sprites.len());
        let mut fits = true;
        for s in sprites {
            if x + s.width > side {
                x = 0;
                y += row;
                row = 0;
            }
            if y + s.height > side {
                fits = false;
                break;
            }
            pos.push((x, y));
            x += s.width;
            row = row.max(s.height);
        }
        if !fits {
            side *= 2;
            continue;
        }
        let mut rgba = vec![0u8; (side * side * 4) as usize];
        let mut rects = Vec::with_capacity(sprites.len());
        for (s, &(px, py)) in sprites.iter().zip(&pos) {
            for r in 0..s.height {
                let src = (r * s.width * 4) as usize;
                let dst = (((py + r) * side + px) * 4) as usize;
                rgba[dst..dst + (s.width * 4) as usize].copy_from_slice(&s.rgba[src..src + (s.width * 4) as usize]);
            }
            let f = side as f32;
            rects.push([px as f32 / f, py as f32 / f, s.width as f32 / f, s.height as f32 / f]);
        }
        return (Image { width: side, height: side, rgba }, rects);
    }
}

/// Special glint coordinates (compasses, clock) before the scrolling matrix: the game
/// projects the vertex position in the model frame (fraction of a block), divided by
/// the context scale `k` (0.5 in the interface, 0.75 in first person, 1 elsewhere), onto
/// the quad's face: half turn around y, quarter turn back around x, rotation specific
/// to the face, then (−x, −y) / 128. The result depends neither on the camera nor on the pose.
/// Same table in `entity.wgsl` (vertex shader).
pub(crate) fn special_glint_uv(p: [f32; 3], dir: rc_assets::geom::Dir, k: f32) -> [f32; 2] {
    use rc_assets::geom::Dir;
    let [x, y, z] = p.map(|c| c / k);
    let uv = match dir {
        Dir::Up => [x, z],
        Dir::Down => [x, -z],
        Dir::North => [-x, -y],
        Dir::South => [x, -y],
        Dir::West => [-z, -y],
        Dir::East => [z, -y],
    };
    uv.map(|c| c / 128.0)
}

fn argb(c: u32) -> [f32; 4] {
    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
    [ch(16), ch(8), ch(0), ch(24)]
}

/// Inverse transpose (up to a positive factor) of a 3 × 3 matrix: cofactors divided
/// by the sign of the determinant.
fn normal_matrix(m: &Mat4) -> [[f32; 3]; 3] {
    let a = |i: usize, j: usize| m[i][j];
    let c = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        a(r0, c0) * a(r1, c1) - a(r0, c1) * a(r1, c0)
    };
    let cof: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| c(i, j)));
    let det = a(0, 0) * cof[0][0] + a(0, 1) * cof[0][1] + a(0, 2) * cof[0][2];
    let s = if det < 0.0 { -1.0 } else { 1.0 };
    cof.map(|r| r.map(|v| v * s))
}

impl ItemAtlas {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut models: ItemModels,
        mut special: crate::item_special::SpecialAssets,
    ) -> Self {
        // Textures of the special models following the sprites of the models.
        let special_base = models.sprites.len();
        let mut all = std::mem::take(&mut models.sprites);
        all.append(&mut special.images);
        let head_base = all.len();
        all.extend((0..HEAD_SLOTS).map(|_| Image { width: 64, height: 64, rgba: vec![0; 64 * 64 * 4] }));
        let (sheet, rects) = pack(&all);
        let sheet_side = sheet.width;
        drop(all);
        let sprites = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("item sprites"),
                size: wgpu::Extent3d { width: sheet.width, height: sheet.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &sheet.rgba,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("item sprites"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // Images sent to the GPU: not read again afterwards (baking is finished at load), except
        // those of the animated sprites, rewritten into the sheet as ticks go by.
        let anims: Vec<(usize, crate::sprite_anim::SpriteAnim)> = std::mem::take(&mut models.frames)
            .into_iter()
            .zip(&models.animations)
            .enumerate()
            .filter_map(|(i, (frames, a))| {
                let a = a.as_ref()?;
                crate::sprite_anim::SpriteAnim::new(frames.into_iter().map(|f| vec![f]).collect(), a).map(|s| (i, s))
            })
            .collect();
        models.sprites = Vec::new();
        let glint_img = models.glint.take().unwrap_or_else(|| Image::missing(16));
        let glint = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("enchantment glint"),
                size: wgpu::Extent3d { width: glint_img.width, height: glint_img.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &glint_img.rgba,
        );
        // Glint: linear filtering, repeated texture (`.mcmeta` blur).
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("enchantment glint"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("item atlas"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("items"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
                        view_dimension: wgpu::TextureViewDimension::D2,
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
        let sprite_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("items"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&sprites.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&glint.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&repeat) },
            ],
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("item.wgsl"));
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("items"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |fs: &str, blend: Option<wgpu::BlendState>, compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("items"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32, 4 => Float32x2],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Translucent: rgb = src·αs + dst·(1 − αs), α = αs + αd·(1 − αs).
        let over = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let solid = pipeline("fs", None, wgpu::CompareFunction::GreaterEqual);
        let translucent = pipeline("fs", Some(over), wgpu::CompareFunction::GreaterEqual);
        let clear = pipeline("fs_clear", None, wgpu::CompareFunction::Always);
        let mut anim_sprite = vec![false; rects.len()];
        for (i, _) in &anims {
            if let Some(a) = anim_sprite.get_mut(*i) {
                *a = true;
            }
        }
        let started = std::time::Instant::now();
        ItemAtlas {
            models,
            rects,
            sprite_bind,
            solid,
            translucent,
            clear,
            uniform,
            started,
            glint: (0.5, 0.75),
            target: None,
            size: 0,
            cell: 0,
            slots: HashMap::new(),
            free: Vec::new(),
            next: 0,
            clears: Vec::new(),
            solids: Vec::new(),
            blended: Vec::new(),
            vb: None,
            special,
            special_base,
            sheet: sprites,
            sheet_side,
            head_base,
            head_next: 0,
            head_slots: HashMap::new(),
            anims,
            recreated: false,
            generation: 0,
            opts: AtlasOpts::from_env(),
            last_clock: started,
            clock_due: false,
            tick_no: 0,
            stamp: vec![0; anim_sprite.len()],
            anim_sprite,
            all: Vec::new(),
            squads: Vec::new(),
            #[cfg(test)]
            fake_now: None,
        }
    }

    pub fn view(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref().map(|t| &t.1)
    }

    /// Current instant (forced in tests).
    fn now(&self) -> std::time::Instant {
        #[cfg(test)]
        if let Some(d) = self.fake_now {
            return self.started + d;
        }
        std::time::Instant::now()
    }

    fn capacity(&self) -> u32 {
        (self.size / self.cell.max(1)).pow(2)
    }

    /// Start of frame: GUI scale (cell size) and freed animated cells.
    pub fn begin(&mut self, device: &wgpu::Device, gui_scale: u32) {
        let cell = 16 * gui_scale.max(1);
        if cell != self.cell || self.target.is_none() {
            self.resize(device, cell, 512.max(cell * 8));
        }
        if self.opts.glint_clock {
            let now = self.now();
            self.clock_due = now.duration_since(self.last_clock).as_millis() >= ANIMATED_MS;
            if self.clock_due {
                self.last_clock = now;
            }
        }
        if self.opts.borrow {
            // A single walk, freed cells placed directly (same order as below).
            let free = &mut self.free;
            self.slots.retain(|_, s| {
                let keep = s.used;
                s.used = false;
                if !keep {
                    free.push(s.cell);
                }
                keep
            });
            return;
        }
        let mut freed = Vec::new();
        self.slots.retain(|_, s| {
            if !s.used {
                freed.push(s.cell);
            }
            s.used
        });
        self.free.extend(freed);
        for s in self.slots.values_mut() {
            s.used = false;
        }
    }

    fn resize(&mut self, device: &wgpu::Device, cell: u32, size: u32) {
        let limit = device.limits().max_texture_dimension_2d;
        let size = size.next_power_of_two().min(limit);
        let make = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = make(
            "item atlas",
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | if cfg!(test) { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
        );
        let depth =
            make("item depth", wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let (cv, dv) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
        self.target = Some((color, cv, dv));
        self.size = size;
        self.cell = cell;
        self.slots.clear();
        self.free.clear();
        self.next = 0;
        self.recreated = true;
        self.generation += 1;
    }

    /// Cell of a render (`key` = identity); drawn this frame if it is new or animated.
    /// Returns the cell's UV rectangle in the atlas.
    pub fn cell_for(&mut self, device: &wgpu::Device, key: u64, layers: &[Layer]) -> Option<[f32; 4]> {
        if let Some(s) = self.slots.get_mut(&key) {
            s.used = true;
            let cell = s.cell;
            if !s.animated {
                return Some(self.uv(cell));
            }
            let now = self.now();
            let (opts, due, top, tick, stamp) = (self.opts, self.clock_due, self.last_clock, self.tick_no, &self.stamp);
            let s = self.slots.get_mut(&key).expect("cell seen above");
            // Shared clock: a cell already drawn since the beat (same key requested several
            // times in the frame, or new cell) is not redrawn, as with the per-cell clock.
            let clock = |s: &Slot| {
                if opts.glint_clock {
                    due && s.drawn < top
                } else {
                    now.duration_since(s.drawn).as_millis() >= ANIMATED_MS
                }
            };
            let redraw = if opts.anim_dirty {
                (s.glint && clock(s)) || s.sprites.iter().any(|&i| stamp[i as usize] > s.drawn_tick)
            } else {
                clock(s)
            };
            if redraw {
                s.drawn = now;
                s.drawn_tick = tick;
                self.draw_cell(cell, layers);
            }
            return Some(self.uv(cell));
        }
        let cell = match self.free.pop() {
            Some(c) => c,
            None if self.next < self.capacity() => {
                self.next += 1;
                self.next - 1
            }
            None => {
                // Full: atlas twice as big (all the cells are redone).
                let (c, s) = (self.cell, self.size * 2);
                if s > device.limits().max_texture_dimension_2d {
                    return None;
                }
                self.resize(device, c, s);
                self.next = 1;
                0
            }
        };
        let glint = layers.iter().any(|l| l.glint != 0);
        let animated = glint || layers.iter().any(|l| self.baked(l.model).animated);
        let mut sprites = Vec::new();
        if self.opts.anim_dirty && animated {
            for l in layers {
                for q in self.baked(l.model).quads() {
                    if self.anim_sprite.get(q.sprite as usize) == Some(&true) && !sprites.contains(&q.sprite) {
                        sprites.push(q.sprite);
                    }
                }
            }
        }
        let slot = Slot {
            cell,
            used: true,
            animated,
            glint,
            sprites: sprites.into(),
            drawn: self.now(),
            drawn_tick: self.tick_no,
        };
        self.slots.insert(key, slot);
        self.draw_cell(cell, layers);
        Some(self.uv(cell))
    }

    fn baked(&self, model: u32) -> &BakedItem {
        self.models.baked(model).unwrap_or(&self.models.missing)
    }

    fn uv(&self, cell: u32) -> [f32; 4] {
        let per = self.size / self.cell;
        let (x, y) = (cell % per, cell / per);
        let f = self.size as f32;
        [(x * self.cell) as f32 / f, (y * self.cell) as f32 / f, self.cell as f32 / f, self.cell as f32 / f]
    }

    fn draw_cell(&mut self, cell: u32, layers: &[Layer]) {
        let per = self.size / self.cell;
        let s = self.cell as f32;
        let (cx, cy) = ((cell % per) as f32 * s, (cell / per) as f32 * s);
        // Clearing the cell: color 0, depth 0 (z = −1000).
        let r = [[cx, cy], [cx, cy + s], [cx + s, cy + s], [cx + s, cy]];
        for i in [0, 1, 2, 2, 3, 0] {
            self.clears.push(Vertex {
                pos: [r[i][0], r[i][1], -1000.0],
                uv: [0.0; 2],
                color: [0.0; 4],
                glint: 0.0,
                guv: [0.0; 2],
            });
        }
        let Some(first) = layers.first() else { return };
        let lights = if self.baked(first.model).gui_front { LIGHTS_FRONT } else { LIGHTS_SIDE };
        let base = mat_mul(&translate([cx + s / 2.0, cy + s / 2.0, 0.0]), &scale([s, -s, s]));
        for layer in layers {
            let model = mat_mul(&self.baked(layer.model).display, &layer.local);
            let pose = mat_mul(&base, &model);
            let nm = normal_matrix(&mat_mul(&scale([1.0, -1.0, 1.0]), &model));
            if layer.special != 0 {
                self.draw_special(layer, &pose, &nm, lights);
            }
            // Old path (`borrow` off): model copy, quads unfolded again.
            let owned;
            let baked = if self.opts.borrow {
                self.models.baked(layer.model).unwrap_or(&self.models.missing)
            } else {
                owned = self.baked(layer.model).clone();
                &owned
            };
            for q in baked.quads() {
                let p = q.pos.map(|v| apply(&pose, v));
                // Back faces culled (direction of the vertices on screen, y downward).
                let cross = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                if cross >= 0.0 {
                    continue;
                }
                let d = q.dir.normal();
                let n = std::array::from_fn::<f32, 3, _>(|i| nm[i][0] * d[0] + nm[i][1] * d[1] + nm[i][2] * d[2]);
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(f32::MIN_POSITIVE);
                let n = n.map(|c| c / len);
                let dot = |l: [f32; 3]| (l[0] * n[0] + l[1] * n[1] + l[2] * n[2]).max(0.0);
                let k = (0.6 * (dot(lights[0]) + dot(lights[1])) + 0.4).min(1.0);
                let tint =
                    usize::try_from(q.tint).ok().and_then(|t| layer.tints.get(t)).copied().unwrap_or(0xffff_ffff);
                let c = argb(tint);
                let color = [c[0] * k, c[1] * k, c[2] * k, c[3]];
                let rect = self.rects[q.sprite as usize];
                let v: [Vertex; 4] = std::array::from_fn(|i| Vertex {
                    pos: p[i],
                    uv: [rect[0] + q.uv[i][0] * rect[2], rect[1] + q.uv[i][1] * rect[3]],
                    color,
                    glint: f32::from(layer.glint.min(2)),
                    guv: if layer.glint == 2 { special_glint_uv(q.pos[i], q.dir, 0.5) } else { [0.0; 2] },
                });
                let tri = [0, 1, 2, 2, 3, 0].map(|i| v[i]);
                if q.translucent {
                    let z = p.iter().map(|v| v[2]).sum::<f32>() / 4.0;
                    self.blended.push((z, tri));
                } else {
                    self.solids.extend_from_slice(&tri);
                }
            }
        }
    }

    /// Skin (64 × 64) of a player head that arrived: written into a reserved slot of the
    /// sheet, used from the next frame by the icons of this head.
    /// One tick of the texture animation: the current frame of each animated sprite that changes
    /// is rewritten into the sheet (animated cells redraw themselves).
    pub fn tick_textures(&mut self, queue: &wgpu::Queue) {
        let side = self.sheet_side as f32;
        self.tick_no += 1;
        for (i, anim) in &mut self.anims {
            let Some(r) = self.rects.get(*i) else { continue };
            let (w, h) = ((r[2] * side).round() as u32, (r[3] * side).round() as u32);
            if !anim.tick() {
                continue;
            }
            let Some(img) = anim.current(w).filter(|img| img.height == h) else { continue };
            if let Some(t) = self.stamp.get_mut(*i) {
                *t = self.tick_no;
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.sheet,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: (r[0] * side).round() as u32, y: (r[1] * side).round() as u32, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
    }

    pub fn set_head_skin(&mut self, queue: &wgpu::Queue, key: u128, img: &Image) {
        if img.width != 64 || img.height != 64 {
            return;
        }
        let slot = match self.head_slots.get(&key) {
            Some(&s) => s,
            None if self.head_next < HEAD_SLOTS => {
                self.head_next += 1;
                self.head_slots.insert(key, self.head_next - 1);
                self.head_next - 1
            }
            None => return,
        };
        let Some(r) = self.rects.get(self.head_base + slot) else { return };
        let side = self.sheet_side as f32;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.sheet,
                mip_level: 0,
                origin: wgpu::Origin3d { x: (r[0] * side).round() as u32, y: (r[1] * side).round() as u32, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &img.rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(64 * 4), rows_per_image: Some(64) },
            wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
        );
        // Cells already drawn with the default skin are redrawn.
        self.slots.clear();
        self.free.clear();
        self.next = 0;
    }

    /// Geometry of a special model (`item_special.rs`), same lighting as the baked quads;
    /// opaque quads (cutout), in order (the patterns cover the background on ties).
    fn draw_special(&mut self, layer: &Layer, pose: &Mat4, nm: &[[f32; 3]; 3], lights: [[f32; 3]; 2]) {
        let mut quads = if self.opts.borrow { std::mem::take(&mut self.squads) } else { Vec::new() };
        quads.clear();
        self.special.quads(layer.special, &layer.tints, &mut quads);
        // Player head whose skin arrived: its slot replaces the default skin.
        let skin = (layer.special == 2)
            .then(|| crate::item_special::head_key_of(&layer.tints))
            .flatten()
            .and_then(|k| self.head_slots.get(&k))
            .and_then(|&s| self.rects.get(self.head_base + s).copied());
        for q in &quads {
            let p = q.pos.map(|v| apply(pose, v));
            let cross = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            if cross >= 0.0 {
                continue;
            }
            let d = q.normal;
            let n = std::array::from_fn::<f32, 3, _>(|i| nm[i][0] * d[0] + nm[i][1] * d[1] + nm[i][2] * d[2]);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(f32::MIN_POSITIVE);
            let n = n.map(|c| c / len);
            let dot = |l: [f32; 3]| (l[0] * n[0] + l[1] * n[1] + l[2] * n[2]).max(0.0);
            let k = (0.6 * (dot(lights[0]) + dot(lights[1])) + 0.4).min(1.0);
            let c = argb(q.tint);
            let color = [c[0] * k, c[1] * k, c[2] * k, c[3]];
            let Some(rect) = skin.or_else(|| self.rects.get(self.special_base + q.image).copied()) else { continue };
            let v: [Vertex; 4] = std::array::from_fn(|i| Vertex {
                pos: p[i],
                uv: [rect[0] + q.uv[i][0] * rect[2], rect[1] + q.uv[i][1] * rect[3]],
                color,
                // Special models: never a compass or the clock (standard glint).
                glint: f32::from(layer.glint != 0),
                guv: [0.0; 2],
            });
            self.solids.extend_from_slice(&[0, 1, 2, 2, 3, 0].map(|i| v[i]));
        }
        if self.opts.borrow {
            self.squads = quads;
        }
    }

    /// Draws the pending cells into the atlas (before the frame's pass).
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        if self.clears.is_empty() {
            return;
        }
        let Some((_, color, depth)) = &self.target else { return };
        // Glint scrolling (`items.md` part 2 § 7.3): speed and intensity settings.
        let (speed, strength) = self.glint;
        let t = self.now().duration_since(self.started);
        let ms = (t.as_secs_f64() * 1000.0 * f64::from(speed) * 8.0).floor() as i64;
        let (o0, o1) = ((ms % 110_000) as f32 / 110_000.0, (ms % 30_000) as f32 / 30_000.0);
        let (s, c) = (std::f32::consts::PI / 18.0).sin_cos();
        // TM = T(−o0, o1) · Rz(10°) · S(8): its first two rows (u, v, 1).
        let u: [f32; 12] =
            [self.size as f32, strength, 0.0, 0.0, 8.0 * c, -8.0 * s, -o0, 0.0, 8.0 * s, 8.0 * c, o1, 0.0];
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&u));
        self.blended.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (n_clear, n_solid) = (self.clears.len() as u32, self.solids.len() as u32);
        if self.opts.borrow {
            // Kept buffers: no capacity lost from one frame to the next.
            self.all.clear();
            self.all.append(&mut self.clears);
        } else {
            self.all = std::mem::take(&mut self.clears);
        }
        self.all.append(&mut self.solids);
        self.all.extend(self.blended.drain(..).flat_map(|(_, t)| t));
        let all = &self.all;
        let bytes: &[u8] = bytemuck::cast_slice(all);
        if self.vb.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64) {
            let cap = (bytes.len() as u64).next_power_of_two();
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("items"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vb = Some((b, cap));
        }
        let vb = &self.vb.as_ref().expect("buffer created above").0;
        queue.write_buffer(vb, 0, bytes);
        let first = std::mem::replace(&mut self.recreated, false);
        let load = |c| if first { wgpu::LoadOp::Clear(c) } else { wgpu::LoadOp::Load };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("item atlas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: load(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: if first { wgpu::LoadOp::Clear(0.0) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: timer.and_then(|t| t.pass("item atlas", crate::gpu_time::Role::Inner)),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.sprite_bind, &[]);
        pass.set_vertex_buffer(0, vb.slice(..bytes.len() as u64));
        pass.set_pipeline(&self.clear);
        pass.draw(0..n_clear, 0..1);
        pass.set_pipeline(&self.solid);
        pass.draw(n_clear..n_clear + n_solid, 0..1);
        pass.set_pipeline(&self.translucent);
        pass.draw(n_clear + n_solid..all.len() as u32, 0..1);
    }
}
