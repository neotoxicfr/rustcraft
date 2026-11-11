//! GUI item atlas (`docs/specs/items.md` part 2 § 5-8): each distinct item render
//! is drawn once in 3D into a cell of `16 · scale` pixels (orthographic projection,
//! reversed depth, item lighting), then the cell is copied into the interface.
//!
//! The cells sample a private sprite sheet. Its animated sprites follow the texture ticks: with
//! `RUSTCRAFT_OPT_ANIMATE_VISIBLE_SPRITES_ONLY` a tick only advances the playbacks, and the current
//! frame of a sprite is blended and written into the sheet when a cell that reads it is drawn
//! (before the atlas pass); a sprite no drawn cell reads costs nothing.

use hashbrown::HashMap;

use rc_assets::item::{BakedItem, ItemModels, Layer, Mat4, apply, mat_mul, scale, translate};
use rc_assets::texture::Image;
use wgpu::util::DeviceExt;

use crate::render::upload::Uploader;
use crate::render::upload::per_frame::FrameBuffer;
use crate::sprite_anim::{FramePixels, SpriteAnim};

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
    /// `RUSTCRAFT_OPT_ANIMATE_VISIBLE_SPRITES_ONLY`: a texture tick only advances the playbacks; a
    /// sprite's current frame is blended and written when a drawn cell reads it, through the
    /// uploader. `=0`: every changed sprite blended and written by the queue on every tick.
    pub visible_sprites_only: bool,
    /// `RUSTCRAFT_OPT_ITEM_ANIM_PIXEL_RECTS`: the pixel rectangle of each animated sprite converted
    /// once at creation instead of rounded from its fractions on every tick.
    pub pixel_rects: bool,
    /// `RUSTCRAFT_OPT_SPRITE_BLEND_BUFFER`: frames read through one kept blend buffer (a plain
    /// frame borrowed from its strip) instead of a new image per sprite and per write.
    pub blend_buffer: bool,
    /// `RUSTCRAFT_OPT_INDEXED_ITEM_ATLAS`: 4 vertices per quad and a static index buffer instead
    /// of 6 vertices (a third fewer bytes per drawn cell), same triangles in the same order.
    pub indexed: bool,
}

impl AtlasOpts {
    #[cfg(test)]
    pub const OLD: AtlasOpts = AtlasOpts {
        borrow: false,
        glint_clock: false,
        anim_dirty: false,
        visible_sprites_only: false,
        pixel_rects: false,
        blend_buffer: false,
        indexed: false,
    };
    #[cfg(test)]
    pub const ALL: AtlasOpts = AtlasOpts {
        borrow: true,
        glint_clock: true,
        anim_dirty: true,
        visible_sprites_only: true,
        pixel_rects: true,
        blend_buffer: true,
        indexed: true,
    };

    pub fn from_env() -> AtlasOpts {
        let on = |v: Option<&str>| v != Some("0");
        AtlasOpts {
            borrow: on(rc_data::env_str!("RUSTCRAFT_OPT_ATLASBORROW")),
            glint_clock: on(rc_data::env_str!("RUSTCRAFT_OPT_GLINTCLOCK")),
            anim_dirty: on(rc_data::env_str!("RUSTCRAFT_OPT_ANIMDIRTY")),
            visible_sprites_only: rc_data::opt_switch!("RUSTCRAFT_OPT_ANIMATE_VISIBLE_SPRITES_ONLY"),
            pixel_rects: rc_data::opt_switch!("RUSTCRAFT_OPT_ITEM_ANIM_PIXEL_RECTS"),
            blend_buffer: rc_data::opt_switch!("RUSTCRAFT_OPT_SPRITE_BLEND_BUFFER"),
            indexed: rc_data::opt_switch!("RUSTCRAFT_OPT_INDEXED_ITEM_ATLAS"),
        }
    }
}

/// Rectangle of a sprite in the sheet, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl PixelRect {
    /// From a rectangle in fractions of a sheet of `sheet_side` pixels (`pack` divided whole
    /// pixels by that side, a power of two: the rounding gives them back exactly).
    fn from_fractions(rect: [f32; 4], sheet_side: f32) -> Self {
        let [x, y, width, height] = rect.map(|fraction| (fraction * sheet_side).round() as u32);
        PixelRect { x, y, width, height }
    }
}

/// Animated sprite of the sheet.
struct AnimatedSprite {
    /// Index of the sprite (in `rects`, and `ItemQuad::sprite` of the quads that read it).
    sprite: usize,
    /// Its rectangle in the sheet (`pixel_rects`).
    rect: PixelRect,
    playback: SpriteAnim,
}

/// Where a write into the sprite sheet goes.
enum SheetWriter<'a> {
    /// wgpu's queue write (a staging buffer made and dropped per call): the path before.
    Queue(&'a wgpu::Queue),
    /// The uploader: its transit ring under `RUSTCRAFT_OPT_RING_TEXTURE_WRITES`, else the queue.
    Uploader { queue: &'a wgpu::Queue, up: &'a mut Uploader },
}

impl SheetWriter<'_> {
    /// Writes the pixels `rgba` of the rectangle `rect` of the sheet (rows back to back).
    fn write(&mut self, sheet: &wgpu::Texture, rect: PixelRect, rgba: &[u8]) {
        let target = wgpu::TexelCopyTextureInfo {
            texture: sheet,
            mip_level: 0,
            origin: wgpu::Origin3d { x: rect.x, y: rect.y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        };
        let rows = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(rect.width * 4),
            rows_per_image: Some(rect.height),
        };
        let size = wgpu::Extent3d { width: rect.width, height: rect.height, depth_or_array_layers: 1 };
        match self {
            SheetWriter::Queue(queue) => queue.write_texture(target, rgba, rows, size),
            SheetWriter::Uploader { queue, up } => up.write_texture(queue, target, rgba, rows, size),
        }
    }
}

/// Bytes of the atlas pass uniform (`Atlas` in `item.wgsl`: three `vec4<f32>`).
const UNIFORM_BYTES: u64 = 48;

/// Corners of a quad in the order of its two triangles: what the index buffer repeats for each
/// quad, and what the path without indices writes out vertex by vertex.
const TRIANGLE_CORNERS: [u32; 6] = [0, 1, 2, 2, 3, 0];

/// Adds a quad to a vertex list: its 4 corners (drawn through the index buffer), or its two
/// triangles written out (6 vertices, `indexed` off).
fn push_quad(vertices: &mut Vec<Vertex>, corners: &[Vertex; 4], indexed: bool) {
    if indexed {
        vertices.extend_from_slice(corners);
    } else {
        vertices.extend_from_slice(&TRIANGLE_CORNERS.map(|corner| corners[corner as usize]));
    }
}

/// Index buffer drawing `quads` quads of 4 corners as two triangles each.
fn quad_index_buffer(device: &wgpu::Device, quads: u32) -> wgpu::Buffer {
    let indices: Vec<u32> = (0..quads).flat_map(|quad| TRIANGLE_CORNERS.map(|corner| 4 * quad + corner)).collect();
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("items (index)"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    })
}

pub struct ItemAtlas {
    pub models: ItemModels,
    /// Rectangle (x, y, w, h) of each sprite in the sprite sheet, in fractions.
    rects: Vec<[f32; 4]>,
    sprite_bind: wgpu::BindGroup,
    solid: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    clear: wgpu::RenderPipeline,
    /// Atlas side, glint strength and glint scrolling: rewritten for every atlas pass.
    uniform: FrameBuffer,
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
    /// Quads to draw this frame: clears, solids, translucents with their depth (4 vertices per
    /// quad with `indexed`, 6 otherwise; the translucent ones always as 4 corners).
    clears: Vec<Vertex>,
    solids: Vec<Vertex>,
    blended: Vec<(f32, [Vertex; 4])>,
    /// Vertices of the atlas pass, rewritten for every pass.
    vertex_buffer: FrameBuffer,
    /// Static indices of the quads and number of quads they cover (`indexed`).
    quad_indices: Option<(wgpu::Buffer, u32)>,
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
    /// Animated sprites, by rising sprite index: rewritten into the sheet when they change.
    animated_sprites: Vec<AnimatedSprite>,
    /// Per sprite index: the sheet is behind the playback (`visible_sprites_only`: changed since
    /// its last write).
    stale_in_sheet: Vec<bool>,
    /// Stale sprites read by a cell drawn this frame: written before the atlas pass.
    sprites_to_upload: Vec<u32>,
    /// Kept buffer the blended frames are written into (`blend_buffer`).
    blend_buffer: Vec<u8>,
    /// Bytes of animated frames written into the sheet (counted by the tests and benches).
    #[cfg(test)]
    uploaded_sprite_bytes: u64,
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
    sprite_changed_tick: Vec<u64>,
    /// Kept buffers (`borrow`): pass vertices, quads of a special model.
    all: Vec<Vertex>,
    squads: Vec<crate::item_special::SQuad>,
    /// Test fault: the stale sprites of the drawn cells are not written (the proofs must see it).
    #[cfg(test)]
    sabotage_skip_stale_sprites: bool,
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
        let animated_sprites: Vec<AnimatedSprite> = std::mem::take(&mut models.frames)
            .into_iter()
            .zip(&models.animations)
            .enumerate()
            .filter_map(|(sprite, (frames, animation))| {
                let playback = SpriteAnim::new(frames.into_iter().map(|f| vec![f]).collect(), animation.as_ref()?)?;
                let rect = PixelRect::from_fractions(*rects.get(sprite)?, sheet_side as f32);
                Some(AnimatedSprite { sprite, rect, playback })
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
        let mut uniform = FrameBuffer::new("item atlas", wgpu::BufferUsages::UNIFORM, UNIFORM_BYTES);
        // Made now: the binding below reads it before the first write.
        uniform.reserve(device, UNIFORM_BYTES);
        let uniform_buffer = uniform.buffer().expect("reserved above");
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
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
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
        for animated in &animated_sprites {
            anim_sprite[animated.sprite] = true;
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
            vertex_buffer: FrameBuffer::new("items", wgpu::BufferUsages::VERTEX, 0),
            quad_indices: None,
            special,
            special_base,
            sheet: sprites,
            sheet_side,
            head_base,
            head_next: 0,
            head_slots: HashMap::new(),
            animated_sprites,
            stale_in_sheet: vec![false; anim_sprite.len()],
            sprites_to_upload: Vec::new(),
            blend_buffer: Vec::new(),
            #[cfg(test)]
            uploaded_sprite_bytes: 0,
            recreated: false,
            generation: 0,
            opts: AtlasOpts::from_env(),
            last_clock: started,
            clock_due: false,
            tick_no: 0,
            sprite_changed_tick: vec![0; anim_sprite.len()],
            anim_sprite,
            all: Vec::new(),
            squads: Vec::new(),
            #[cfg(test)]
            sabotage_skip_stale_sprites: false,
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
            let (opts, due, top, tick) = (self.opts, self.clock_due, self.last_clock, self.tick_no);
            let changed_tick = &self.sprite_changed_tick;
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
                (s.glint && clock(s)) || s.sprites.iter().any(|&i| changed_tick[i as usize] > s.drawn_tick)
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
        let clear_corners = [[cx, cy], [cx, cy + s], [cx + s, cy + s], [cx + s, cy]].map(|[x, y]| Vertex {
            pos: [x, y, -1000.0],
            uv: [0.0; 2],
            color: [0.0; 4],
            glint: 0.0,
            guv: [0.0; 2],
        });
        push_quad(&mut self.clears, &clear_corners, self.opts.indexed);
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
                // This quad samples its sprite in the coming pass: a frame the sheet does not
                // hold yet is written first (set only with `visible_sprites_only`).
                if self.stale_in_sheet[q.sprite as usize] {
                    self.stale_in_sheet[q.sprite as usize] = false;
                    self.sprites_to_upload.push(q.sprite);
                }
                let v: [Vertex; 4] = std::array::from_fn(|i| Vertex {
                    pos: p[i],
                    uv: [rect[0] + q.uv[i][0] * rect[2], rect[1] + q.uv[i][1] * rect[3]],
                    color,
                    glint: f32::from(layer.glint.min(2)),
                    guv: if layer.glint == 2 { special_glint_uv(q.pos[i], q.dir, 0.5) } else { [0.0; 2] },
                });
                if q.translucent {
                    let z = p.iter().map(|v| v[2]).sum::<f32>() / 4.0;
                    self.blended.push((z, v));
                } else {
                    push_quad(&mut self.solids, &v, self.opts.indexed);
                }
            }
        }
    }

    /// One tick of the texture animation: every playback advances, and the animated cells that
    /// read a changed sprite redraw themselves. The changed sprites are written into the sheet at
    /// once by the queue, or only marked stale and written when a drawn cell reads them
    /// (`visible_sprites_only`).
    // Out of line: its own entry in the sampled profiles instead of a share of the frame's.
    #[inline(never)]
    pub fn tick_textures(&mut self, queue: &wgpu::Queue) {
        self.tick_no += 1;
        for index in 0..self.animated_sprites.len() {
            let rect = self.rect_of_animated(index);
            let animated = &mut self.animated_sprites[index];
            if !animated.playback.tick() {
                continue;
            }
            let sprite = animated.sprite;
            if self.opts.visible_sprites_only {
                // Same condition as a write that succeeds: a cell redraws in the same cases.
                if animated.playback.has_frame(rect.width, rect.height) {
                    self.sprite_changed_tick[sprite] = self.tick_no;
                    self.stale_in_sheet[sprite] = true;
                }
            } else if self.write_current_frame(index, rect, &mut SheetWriter::Queue(queue)) {
                self.sprite_changed_tick[sprite] = self.tick_no;
            }
        }
    }

    /// Rectangle in the sheet of the animated sprite number `index`: the one converted at
    /// creation, or its fractions rounded again (`pixel_rects` off, the path before).
    fn rect_of_animated(&self, index: usize) -> PixelRect {
        let animated = &self.animated_sprites[index];
        if self.opts.pixel_rects {
            animated.rect
        } else {
            PixelRect::from_fractions(self.rects[animated.sprite], self.sheet_side as f32)
        }
    }

    /// Writes the current frame of the animated sprite number `index` into its rectangle of the
    /// sheet; false if the playback has no frame of that size.
    fn write_current_frame(&mut self, index: usize, rect: PixelRect, writer: &mut SheetWriter<'_>) -> bool {
        let playback = &self.animated_sprites[index].playback;
        let owned_image;
        let frame = if self.opts.blend_buffer {
            playback.current_pixels(rect.width, &mut self.blend_buffer)
        } else {
            owned_image = playback.current(rect.width);
            owned_image.as_ref().map(|image| FramePixels { height: image.height, rgba: &image.rgba })
        };
        let Some(frame) = frame.filter(|frame| frame.height == rect.height) else { return false };
        writer.write(&self.sheet, rect, frame.rgba);
        #[cfg(test)]
        {
            self.uploaded_sprite_bytes += frame.rgba.len() as u64;
        }
        true
    }

    /// Writes into the sheet, before the atlas pass, the current frame of the stale sprites that
    /// the cells drawn this frame read (`visible_sprites_only`).
    // Out of line, like `tick_textures`: the blends moved here keep an entry of their own.
    #[inline(never)]
    fn upload_stale_sprites(&mut self, queue: &wgpu::Queue, up: &mut Uploader) {
        let mut sprites = std::mem::take(&mut self.sprites_to_upload);
        #[cfg(test)]
        if self.sabotage_skip_stale_sprites {
            sprites.clear();
        }
        for sprite in sprites.drain(..) {
            // `animated_sprites` is sorted by sprite index (built in that order).
            let found = self.animated_sprites.binary_search_by_key(&(sprite as usize), |animated| animated.sprite);
            let Ok(index) = found else { continue };
            let rect = self.rect_of_animated(index);
            self.write_current_frame(index, rect, &mut SheetWriter::Uploader { queue, up });
        }
        self.sprites_to_upload = sprites;
    }

    /// Skin (64 × 64) of a player head that arrived: written into a reserved slot of the
    /// sheet, used from the next frame by the icons of this head.
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
        let rect = PixelRect::from_fractions(*r, self.sheet_side as f32);
        // By the queue, where the animated frames may take the uploader's ring: the head slots
        // and the animated sprites never overlap, so the order of the two kinds of write in a
        // submission changes nothing.
        SheetWriter::Queue(queue).write(&self.sheet, rect, &img.rgba);
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
            push_quad(&mut self.solids, &v, self.opts.indexed);
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
        up: &mut Uploader,
        enc: &mut wgpu::CommandEncoder,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        if self.clears.is_empty() || self.target.is_none() {
            return;
        }
        // The sheet first: the pass below samples the sprites of the cells it draws.
        self.upload_stale_sprites(queue, up);
        self.write_pass_uniform(queue, up);
        let (clear_end, solid_end) = self.gather_pass_vertices();
        let indexed = self.opts.indexed;
        if indexed {
            self.cover_quads_with_indices(device, (self.all.len() / 4) as u32);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&self.all);
        self.vertex_buffer.write(up, queue, bytes);
        let first = std::mem::replace(&mut self.recreated, false);
        let load = |c| if first { wgpu::LoadOp::Clear(c) } else { wgpu::LoadOp::Load };
        let (Some((_, color, depth)), Some(vertices)) = (&self.target, self.vertex_buffer.buffer()) else { return };
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
        pass.set_vertex_buffer(0, vertices.slice(..bytes.len() as u64));
        if let Some((indices, _)) = self.quad_indices.as_ref().filter(|_| indexed) {
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        }
        // A range of the pass vertices: as it is, or through the indices of its quads
        // (4 vertices and 6 indices per quad).
        let draw = |pass: &mut wgpu::RenderPass<'_>, vertices: std::ops::Range<u32>| {
            if indexed {
                pass.draw_indexed(vertices.start / 4 * 6..vertices.end / 4 * 6, 0, 0..1);
            } else {
                pass.draw(vertices, 0..1);
            }
        };
        pass.set_pipeline(&self.clear);
        draw(&mut pass, 0..clear_end);
        pass.set_pipeline(&self.solid);
        draw(&mut pass, clear_end..solid_end);
        pass.set_pipeline(&self.translucent);
        draw(&mut pass, solid_end..self.all.len() as u32);
    }

    /// Uniform of the atlas pass: atlas side, glint strength and glint scrolling
    /// (`items.md` part 2 § 7.3: speed and intensity settings).
    fn write_pass_uniform(&mut self, queue: &wgpu::Queue, up: &mut Uploader) {
        let (speed, strength) = self.glint;
        let t = self.now().duration_since(self.started);
        let ms = (t.as_secs_f64() * 1000.0 * f64::from(speed) * 8.0).floor() as i64;
        let (o0, o1) = ((ms % 110_000) as f32 / 110_000.0, (ms % 30_000) as f32 / 30_000.0);
        let (s, c) = (std::f32::consts::PI / 18.0).sin_cos();
        // TM = T(−o0, o1) · Rz(10°) · S(8): its first two rows (u, v, 1).
        let u: [f32; 12] =
            [self.size as f32, strength, 0.0, 0.0, 8.0 * c, -8.0 * s, -o0, 0.0, 8.0 * s, 8.0 * c, o1, 0.0];
        self.uniform.write(up, queue, bytemuck::cast_slice(&u));
    }

    /// Moves the frame's quads into `all` in drawing order (clears, solids, translucents from the
    /// farthest); returns the vertex where the clears end and the one where the solids end.
    fn gather_pass_vertices(&mut self) -> (u32, u32) {
        self.blended.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (clears, solids) = (self.clears.len() as u32, self.solids.len() as u32);
        if self.opts.borrow {
            // Kept buffers: no capacity lost from one frame to the next.
            self.all.clear();
            self.all.append(&mut self.clears);
        } else {
            self.all = std::mem::take(&mut self.clears);
        }
        self.all.append(&mut self.solids);
        let indexed = self.opts.indexed;
        for (_, corners) in self.blended.drain(..) {
            push_quad(&mut self.all, &corners, indexed);
        }
        (clears, clears + solids)
    }

    /// Makes the static index buffer cover `quads` quads (grown by powers of two; the indices of
    /// a quad only depend on its rank, so they are never rewritten).
    fn cover_quads_with_indices(&mut self, device: &wgpu::Device, quads: u32) {
        if self.quad_indices.as_ref().is_none_or(|(_, covered)| *covered < quads) {
            let covered = quads.next_power_of_two();
            self.quad_indices = Some((quad_index_buffer(device, covered), covered));
        }
    }
}
