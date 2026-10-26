//! 2D interface: GUI scale, layers, sprites of the interface atlas, text
//! (`docs/specs/hud.md` § 1, `docs/specs/text.md` § 5–6).

use hashbrown::HashMap;

use rc_assets::font::{Font, Glyph};
use rc_assets::gui::{GuiAtlas, Scaling, Sprite};
use rc_text::StyledChar;
use wgpu::util::DeviceExt;

#[cfg(test)]
mod tests_opt;
pub mod text_objects;

/// Entry of the visual-order cache: (text, direction), order, last frame drawn.
type BidiEntry = ((Box<[char]>, bool), std::sync::Arc<[(char, u32)]>, u64);

/// The cache entry is indeed that of this text and direction (not a collision).
fn bidi_entry_is(e: &BidiEntry, chars: &[StyledChar], rtl: bool) -> bool {
    e.0.1 == rtl && e.0.0.len() == chars.len() && e.0.0.iter().zip(chars).all(|(a, s)| *a == s.c)
}

/// Interface resources read at startup.
pub struct GuiAssets {
    pub atlas: GuiAtlas,
    /// Fonts by id (`minecraft:default`, …).
    pub fonts: Vec<(String, Font)>,
    pub lang: rc_text::Lang,
    /// `textures/misc/vignette.png` (sampled bilinearly).
    pub vignette: Option<rc_assets::texture::Image>,
    /// Item definitions and models (GUI item atlas).
    pub items: Option<rc_assets::item::ItemModels>,
    /// Textures and layers of the special models (heads, banners…) of the icons.
    pub special: crate::item_special::SpecialAssets,
}

/// Blend of a stratum (`hud.md`, header): translucent, inversion (crosshair), vignette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Invert,
    Vignette,
    /// Copy of the cells of the item atlas (premultiplied colours).
    Premultiplied,
    /// Pure addition (src + dst, alpha included): blue tint of the text highlight.
    Additive,
    /// Addition weighted by the source alpha (src·αs + dst): logo of the reload screen.
    AdditiveAlpha,
    /// No blending: the source colour and alpha replace the target (portal background
    /// of the loading screen).
    Opaque,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// 1 = glyph (page), 2 = flat fill; the other modes do not leave the interface.
    pub mode: [u32; 2],
}

/// Interface quad sent as one instance (`RUSTCRAFT_OPT_GUIINSTANCE`): the 4 corners, the
/// uv rectangle (top-left corner, bottom-right corner), the top colour (corners 0 and 3) and the bottom
/// one (corners 1 and 2), the mode. The shader redoes the 6 vertices of the two triangles in the order of
/// the corners (0, 1, 2) (2, 3, 0) with the same f32 values: 88 bytes instead of 240.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Inst {
    pub p: [f32; 8],
    pub uv: [f32; 4],
    pub top: [f32; 4],
    pub bottom: [f32; 4],
    pub mode: [u32; 2],
}

impl Inst {
    /// Quad of 4 corners (top-left, bottom-left, bottom-right, top-right) as an instance. Every quad
    /// placed by the interface has uv in a rectangle and one colour per row (vertical gradient),
    /// clipping included (bilinear interpolation keeps these two shapes bit-exact).
    fn of(q: &[Vertex]) -> Inst {
        debug_assert!(q[1].uv[0] == q[0].uv[0] && q[3].uv == [q[2].uv[0], q[0].uv[1]] && q[1].uv[1] == q[2].uv[1]);
        debug_assert!(q[3].color == q[0].color && q[2].color == q[1].color);
        Inst {
            p: [q[0].pos[0], q[0].pos[1], q[1].pos[0], q[1].pos[1], q[2].pos[0], q[2].pos[1], q[3].pos[0], q[3].pos[1]],
            uv: [q[0].uv[0], q[0].uv[1], q[2].uv[0], q[2].uv[1]],
            top: q[0].color,
            bottom: q[1].color,
            mode: q[0].mode,
        }
    }
}

/// Corners of a quad placed in the layers: two triangles (old path) or 4 corners (index or
/// instance).
const TRI: &[usize] = &[0, 1, 2, 2, 3, 0];
const QUAD: &[usize] = &[0, 1, 2, 3];

/// Upload format of the interface quads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// 6 vertices per quad, direct draw (old path).
    Tri,
    /// 4 vertices per quad and static index buffer (`RUSTCRAFT_OPT_GUIINDEX`).
    Indexed,
    /// One instance per quad (`RUSTCRAFT_OPT_GUIINSTANCE`).
    Instanced,
}

/// Interface switches (`RUSTCRAFT_OPT_GUI*`, `RUSTCRAFT_OPT_SPRITEIDS`), read once
/// at startup; `=0` restores the old path (`RUSTCRAFT_OPT_GUIKEEP`: `=1` enables it). The
/// tests change them per instance ([`Gui::set_opts`]) to compare the paths in a single
/// process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiOpts {
    /// Upload written directly into the staging buffer, without an intermediate copy.
    pub write_with: bool,
    /// Screen uniform written only when it changes; interleaved draws without allocation.
    pub uniform: bool,
    /// 4 vertices per quad and static index buffer.
    pub index: bool,
    /// One instance per quad (takes precedence over `index`).
    pub instance: bool,
    /// Only the byte range changed since the previous frame is sent (takes precedence over `write_with`).
    pub keep: bool,
    /// Frequent sprites and uniform rectangles resolved once.
    pub sprite_ids: bool,
    /// Background repeated as one quad (shader mode 7) instead of one quad per tile.
    pub tile_shader: bool,
    /// Vertices of a text line remembered and copied as is when it comes back
    /// identical (`RUSTCRAFT_OPT_TEXTCACHE`).
    pub text_cache: bool,
    /// Glyphs through the flat table of each font, lent without copying, font resolved once
    /// per run of the same style (`RUSTCRAFT_OPT_FONTTABLE`).
    pub font_table: bool,
}

impl GuiOpts {
    /// All the old paths.
    #[cfg(test)]
    pub const OLD: GuiOpts = GuiOpts {
        write_with: false,
        uniform: false,
        index: false,
        instance: false,
        keep: false,
        sprite_ids: false,
        tile_shader: false,
        text_cache: false,
        font_table: false,
    };

    pub fn from_env() -> GuiOpts {
        let on = |v: Option<&str>| v != Some("0");
        GuiOpts {
            write_with: on(rc_data::env_str!("RUSTCRAFT_OPT_GUIWRITEWITH")),
            uniform: on(rc_data::env_str!("RUSTCRAFT_OPT_GUIUNIFORM")),
            index: on(rc_data::env_str!("RUSTCRAFT_OPT_GUIINDEX")),
            instance: on(rc_data::env_str!("RUSTCRAFT_OPT_GUIINSTANCE")),
            // Only switch at 0 by default (=1 enables it): gain in bytes, not yet in time.
            keep: rc_data::env_str!("RUSTCRAFT_OPT_GUIKEEP") == Some("1"),
            sprite_ids: on(rc_data::env_str!("RUSTCRAFT_OPT_SPRITEIDS")),
            tile_shader: on(rc_data::env_str!("RUSTCRAFT_OPT_GUITILESHADER")),
            text_cache: on(rc_data::env_str!("RUSTCRAFT_OPT_TEXTCACHE")),
            font_table: on(rc_data::env_str!("RUSTCRAFT_OPT_FONTTABLE")),
        }
    }

    fn format(&self) -> Format {
        if self.instance {
            Format::Instanced
        } else if self.index {
            Format::Indexed
        } else {
            Format::Tri
        }
    }
}

/// Frequent sprites resolved once (`RUSTCRAFT_OPT_SPRITEIDS`): buttons, tooltip, boss
/// bars, tabs and scroller of the creative inventory. Rank in [`pre_names`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pre(u8);

/// Boss bar colours, in protocol order.
pub const BOSS_COLORS: [&str; 7] = ["pink", "blue", "red", "green", "yellow", "purple", "white"];
/// Boss bar notches (style 0: none).
pub const BOSS_NOTCHES: [&str; 5] = ["", "notched_6", "notched_10", "notched_12", "notched_20"];

impl Pre {
    pub const BUTTON: Pre = Pre(0);
    pub const BUTTON_DISABLED: Pre = Pre(1);
    pub const BUTTON_HIGHLIGHTED: Pre = Pre(2);
    pub const TOOLTIP_BACKGROUND: Pre = Pre(3);
    pub const TOOLTIP_FRAME: Pre = Pre(4);
    pub const CREATIVE_SCROLLER: Pre = Pre(5);
    pub const CREATIVE_SCROLLER_DISABLED: Pre = Pre(6);
    const BOSS: u8 = 7;
    const TABS: u8 = Self::BOSS + 2 * (BOSS_COLORS.len() + BOSS_NOTCHES.len() - 1) as u8;
    const COUNT: usize = Self::TABS as usize + 28;

    /// Background (`progress` false) or fill of the boss bar of colour `color`.
    pub fn boss_color(color: usize, progress: bool) -> Pre {
        Pre(Self::BOSS + 2 * color as u8 + u8::from(progress))
    }

    /// Background or fill of the notches of style `style` (1 to 4).
    pub fn boss_notches(style: usize, progress: bool) -> Pre {
        Pre(Self::BOSS + 2 * (BOSS_COLORS.len() + style - 1) as u8 + u8::from(progress))
    }

    /// Creative inventory tab: top row (`top`), selected or not, column 0 to 6.
    pub fn creative_tab(top: bool, selected: bool, column: usize) -> Pre {
        Pre(Self::TABS + 14 * u8::from(!top) + 7 * u8::from(selected) + column as u8)
    }
}

/// Names of the pre-resolved sprites, at the rank of [`Pre`] (built once).
pub fn pre_names() -> &'static [String] {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(build_pre_names)
}

fn build_pre_names() -> Vec<String> {
    let mut v: Vec<String> = [
        "widget/button",
        "widget/button_disabled",
        "widget/button_highlighted",
        "tooltip/background",
        "tooltip/frame",
        "container/creative_inventory/scroller",
        "container/creative_inventory/scroller_disabled",
    ]
    .map(str::to_owned)
    .into();
    for c in BOSS_COLORS.iter().chain(&BOSS_NOTCHES[1..]) {
        v.push(format!("boss_bar/{c}_background"));
        v.push(format!("boss_bar/{c}_progress"));
    }
    for row in ["top", "bottom"] {
        for sel in ["unselected", "selected"] {
            for col in 1..=7 {
                v.push(format!("container/creative_inventory/tab_{row}_{sel}_{col}"));
            }
        }
    }
    debug_assert_eq!(v.len(), Pre::COUNT);
    v
}

/// Draw interleaved in the interface: first vertex after which it goes, and the draw.
type Between<'a> = (u32, Box<dyn FnOnce(&mut wgpu::RenderPass<'_>) + 'a>);
/// Same without allocation (`RUSTCRAFT_OPT_GUIUNIFORM`).
type BetweenRef<'a> = (u32, &'a mut dyn FnMut(&mut wgpu::RenderPass<'_>));

/// Strikethrough or underline: rectangle, colour, shadow (colour, offset).
type Effect = ([f32; 4], [f32; 4], Option<([f32; 4], f32)>);

/// Sprite drawn with the linear filter (logo of the reload screen).
const SMOOTH_SPRITE: &str = "tex/gui/title/mojangstudios";
/// End sky and portal texture (background of the loading screen to or from the End).
const END_SKY: &str = "tex/environment/end_sky";
const END_PORTAL: &str = "tex/entity/end_portal/end_portal";
const PAGE: u32 = 256;
const MAX_PAGES: u32 = 32;

/// Place of a glyph in the atlas pages.
#[derive(Clone, Copy)]
struct Slot {
    page: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Metrics of a glyph (without its image): what is needed to place it once its place is known.
#[derive(Clone, Copy)]
struct Geo {
    advance: f32,
    offset: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

/// Place of a glyph to draw: without image; resolved (`None`: pages full, nothing drawn);
/// to resolve at the first quad.
enum Place {
    NoImage,
    At(Option<Slot>),
    Later(Glyph),
}

/// Rank of the default font (style without a font).
fn default_font(fonts: &[(String, Font)]) -> Option<usize> {
    fonts.iter().position(|f| f.0 == "minecraft:default")
}

impl Geo {
    fn of(g: &Glyph) -> Geo {
        Geo { advance: g.advance, offset: g.offset, left: g.left, top: g.top, right: g.right, bottom: g.bottom }
    }
}

/// Remembered text line (`RUSTCRAFT_OPT_TEXTCACHE`): exact input (characters, styles that
/// change the drawing, parameters) and vertices produced, copied as is.
struct TextMemo {
    chars: Box<[StyledChar]>,
    params: TextParams,
    verts: Box<[Vertex]>,
    end: f32,
    frame: u64,
}

/// Parameters of a text placement that change its vertices, in bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TextParams {
    xy: [u32; 2],
    color: u32,
    background: u32,
    pose: u32,
    clip: Option<[u32; 4]>,
    corners: usize,
    font_gen: usize,
    /// Shadow, no suspect glyph, given visual order, right-to-left direction.
    flags: u8,
}

/// Same styled character (full equality: click, hover and insertion count too, they
/// cut the style runs that decide where an underline starts), shares compared
/// first by address.
fn same_char(a: &StyledChar, b: &StyledChar) -> bool {
    fn arc<T: PartialEq + ?Sized>(a: &Option<std::sync::Arc<T>>, b: &Option<std::sync::Arc<T>>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => std::sync::Arc::ptr_eq(x, y) || x == y,
            _ => false,
        }
    }
    // Full destructuring: a field added to the style no longer compiles here without being compared.
    let rc_text::CharStyle {
        color,
        shadow_color,
        bold,
        italic,
        underlined,
        strikethrough,
        obfuscated,
        font,
        click,
        hover,
        insertion,
    } = &a.style;
    let t = &b.style;
    a.c == b.c
        && *color == t.color
        && *shadow_color == t.shadow_color
        && (*bold, *italic, *underlined, *strikethrough, *obfuscated)
            == (t.bold, t.italic, t.underlined, t.strikethrough, t.obfuscated)
        && *font == t.font
        && arc(click, &t.click)
        && arc(hover, &t.hover)
        && arc(insertion, &t.insertion)
}

/// Automatic GUI scale: the largest such that the window keeps at least 320 × 240
/// interface pixels (`chat.md`, `hud.md` § 1).
pub fn auto_scale(width: u32, height: u32) -> u32 {
    calculate_scale(0, width, height)
}

/// "Forced Unicode font" (option `forceUnicodeFont`): the GUI scale then goes to the next
/// even value if it is odd (half-pixel Unicode glyphs sharp).
static FORCE_UNICODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_force_unicode(on: bool) {
    FORCE_UNICODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// GUI scale for option `max_scale` (0: automatic): go up as long as the window keeps
/// at least 320 × 240 interface pixels without exceeding the option, then the forced Unicode
/// font makes the scale even (it can then exceed the automatic maximum).
pub fn calculate_scale(max_scale: u32, width: u32, height: u32) -> u32 {
    scale_for(max_scale, width, height, FORCE_UNICODE.load(std::sync::atomic::Ordering::Relaxed))
}

fn scale_for(max_scale: u32, width: u32, height: u32, unicode: bool) -> u32 {
    let mut s = 1;
    while s != max_scale && s < width && s < height && width / (s + 1) >= 320 && height / (s + 1) >= 240 {
        s += 1;
    }
    if unicode && s % 2 != 0 {
        s += 1;
    }
    s
}

/// Projected size of the interface: window ÷ scale, without rounding.
fn projection_size(window_w: u32, window_h: u32, scale: u32) -> [f32; 2] {
    [window_w as f32 / scale as f32, window_h as f32 / scale as f32]
}

#[cfg(test)]
#[test]
fn projection_keeps_the_fraction_layout_rounds_up() {
    let p = projection_size(1366, 768, 3);
    assert_eq!(p, [1366.0 / 3.0, 256.0]);
    assert_eq!(p[0].ceil(), 456.0); // layout: rounded up
    assert!(p[0] < 456.0); // projection: never rounded
}

/// Rectangle of an object in the text whose character starts at `(x, y)`: 8 × 8, top at
/// `y + 7 − 8` (height above the baseline 8, baseline at 7).
fn object_rect(x: f32, y: f32) -> [f32; 4] {
    let top = y + 7.0 - 8.0;
    [x, top, x + 8.0, top + 8.0]
}

#[cfg(test)]
#[test]
fn object_sprite_sits_one_pixel_above_the_glyph_box() {
    assert_eq!(object_rect(10.0, 20.0), [10.0, 19.0, 18.0, 27.0]);
}

#[cfg(test)]
#[test]
fn unicode_font_forces_even_scale() {
    // 1920 × 1080: automatic maximum 4; 1366 × 768: 3.
    assert_eq!(scale_for(0, 1920, 1080, false), 4);
    assert_eq!(scale_for(0, 1366, 768, false), 3);
    assert_eq!(scale_for(0, 1366, 768, true), 4);
    assert_eq!(scale_for(3, 1920, 1080, true), 4);
    assert_eq!(scale_for(1, 1920, 1080, true), 2);
    assert_eq!(scale_for(2, 1920, 1080, true), 2);
    assert_eq!(scale_for(7, 1366, 768, false), 3);
    assert_eq!(scale_for(0, 400, 300, true), 2);
}

/// The last `added` layers go just before layer `at`; the layer marks
/// (blur, overlays, models, tooltip) that followed it are shifted accordingly. The
/// current layer (the last) remains the one from before the interleaving.
fn move_back<'a, T>(layers: &mut [T], at: usize, added: usize, marks: impl Iterator<Item = &'a mut usize>) {
    layers[at..].rotate_right(added);
    for i in marks {
        if *i >= at {
            *i += added;
        }
    }
}

#[cfg(test)]
#[test]
fn ime_composition_goes_under_the_first_tooltip() {
    // Layers: screen, button, tooltip (mark 2), then composition drawn last (2 layers).
    let mut l = ["screen", "button", "tooltip", "top", "frame", "text"];
    let (mut tip, mut model, mut blur) = (2, 1, 3);
    move_back(&mut l, 2, 2, [&mut tip, &mut model, &mut blur].into_iter());
    assert_eq!(l, ["screen", "button", "frame", "text", "tooltip", "top"]);
    assert_eq!((tip, model, blur), (4, 1, 5));
}

/// ARGB colour → floating-point RGBA.
/// Atlas of the glyphs of `layers` pages.
fn glyph_texture(device: &wgpu::Device, layers: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyphs"),
        size: wgpu::Extent3d { width: PAGE, height: PAGE, depth_or_array_layers: layers },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Fixed resources of the main interface binding.
struct BindParts {
    layout: wgpu::BindGroupLayout,
    sprites: wgpu::TextureView,
    sampler: wgpu::Sampler,
    vignette: wgpu::TextureView,
    linear: wgpu::Sampler,
}

impl BindParts {
    fn bind(&self, device: &wgpu::Device, screen: &wgpu::Buffer, glyphs: &wgpu::Texture) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: screen.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.sprites) },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&glyphs.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2Array),
                        ..Default::default()
                    })),
                },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.vignette) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.linear) },
            ],
        })
    }
}

/// Byte range `[lo, hi)` of `cur` to rewrite over `prev` (bounds aligned to 4); `None` if
/// nothing changed. Beyond the end of `prev`, everything is new.
fn changed_range(prev: &[u8], cur: &[u8]) -> Option<(usize, usize)> {
    const STEP: usize = 64;
    let n = prev.len().min(cur.len());
    let first = (0..n.div_ceil(STEP)).find(|&k| {
        let r = k * STEP..((k + 1) * STEP).min(n);
        prev[r.clone()] != cur[r]
    });
    let lo = match first {
        Some(k) => k * STEP + prev[k * STEP..].iter().zip(&cur[k * STEP..]).position(|(a, b)| a != b).unwrap_or(0),
        None if cur.len() > n => n,
        None => return None,
    };
    let hi = if cur.len() > n {
        cur.len()
    } else {
        // Last differing byte: it exists (at least the one at `lo`).
        let last = (lo / STEP..n.div_ceil(STEP)).rev().find(|&k| {
            let r = k * STEP..((k + 1) * STEP).min(n);
            prev[r.clone()] != cur[r]
        });
        let k = last.unwrap_or(lo / STEP);
        let r = k * STEP..((k + 1) * STEP).min(n);
        r.start + prev[r.clone()].iter().zip(&cur[r]).rposition(|(a, b)| a != b).unwrap_or(0) + 1
    };
    Some((lo & !3, hi.next_multiple_of(4).min(cur.len())))
}

pub fn argb(c: u32) -> [f32; 4] {
    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
    [ch(16), ch(8), ch(0), ch(24)]
}

pub struct Gui {
    pipelines: [wgpu::RenderPipeline; 7],
    device: wgpu::Device,
    pub items: Option<crate::item_atlas::ItemAtlas>,
    item_layout: wgpu::BindGroupLayout,
    /// Binding of the item atlas and number of the bound target.
    item_bind: (wgpu::BindGroup, u32),
    /// Vertex ranges of the frame, per blend.
    ranges: Vec<(Blend, std::ops::Range<u32>)>,
    bind: wgpu::BindGroup,
    screen: wgpu::Buffer,
    glyph_tex: wgpu::Texture,
    /// Resources of the main binding (recreated when the glyph atlas grows).
    parts: BindParts,
    /// Changes at every growth of the glyph atlas.
    pub glyph_gen: u32,
    vb: Option<(wgpu::Buffer, u64)>,
    atlas: GuiAtlas,
    /// Objects drawn in the text (atlas sprites, player heads).
    pub objects: text_objects::TextObjects,
    /// Atlas texture (rewritten slots: server icons).
    sprites_tex: wgpu::Texture,
    atlas_size: (f32, f32),
    fonts: Vec<(String, Font)>,
    /// Rank of `minecraft:default` in `fonts`.
    default_font: Option<usize>,
    /// Font set in place (changes with `set_fonts`: distinct glyph keys).
    font_gen: usize,
    pub lang: rc_text::Lang,
    white: Glyph,
    missing: Glyph,
    slots: HashMap<(usize, u32), Slot>,
    /// Pages: (x cursor, y row, shelf height).
    pages: Vec<(u32, u32, u32)>,
    /// Layers: shapes then text, in order of addition.
    layers: Vec<(Blend, Vec<Vertex>, Vec<Vertex>)>,
    /// Buffers of the previous frame's layers, emptied and reused (no allocation per frame).
    spare: Vec<(Vec<Vertex>, Vec<Vertex>)>,
    /// Frame bytes laid end to end for the upload (old path and upload of changes only),
    /// capacity kept from one frame to the next.
    staging: Vec<u8>,
    /// Bytes sent at the previous frame (`keep`); empty: everything to rewrite.
    sent: Vec<u8>,
    opts: GuiOpts,
    /// Corners placed per quad in the layers (6: triangles, 4: index or instance).
    corners: &'static [usize],
    /// Pipelines of the instances (created on demand) and what is needed to create them.
    inst_pipelines: Option<[wgpu::RenderPipeline; 7]>,
    #[cfg_attr(not(test), allow(dead_code))]
    pipe_src: (wgpu::ShaderModule, wgpu::PipelineLayout, wgpu::TextureFormat),
    /// Static index buffer and number of quads covered.
    ib: Option<(wgpu::Buffer, u32)>,
    /// Last screen uniform written (`uniform`); `None`: to be written.
    screen_sent: Option<[f32; 16]>,
    /// Rectangles (atlas fractions) of the End sky, the portal and the smoothed sprite.
    screen_rects: [f32; 12],
    /// Frequent sprites resolved at creation (rank of [`Pre`]).
    pre: Box<[Option<Sprite>]>,
    /// Smoothed sprite resolved at creation.
    smooth: Option<Sprite>,
    pub scale: u32,
    /// Pose scale applied to the following elements (×2 title of the death screen).
    pub pose: f32,
    /// Layout size: window ÷ scale, rounded up (like the game).
    pub width: f32,
    pub height: f32,
    /// Size of the projection: window ÷ exact scale. The game projects the interface onto this
    /// unrounded size; projecting onto `width` stretched the interface by a fraction of a pixel
    /// when the window is not a multiple of the scale (1366 / 3 = 455.33 projected onto 456).
    pub proj: [f32; 2],
    /// Drawable elements of the frame (vertices, indices or instances depending on the format).
    frame_vertices: u32,
    obfuscate_seed: u64,
    /// Visual orders of right-to-left lines already computed (text, direction) ->
    /// (order, last frame); ~5-9 µs per line saved per frame (`bench_visual` bench).
    /// Key: fingerprint of the text and direction; the text is kept to check.
    bidi_cache: HashMap<u64, BidiEntry>,
    /// Remembered text lines (`RUSTCRAFT_OPT_TEXTCACHE`), by input fingerprint.
    text_memo: HashMap<u64, TextMemo>,
    /// Fingerprints seen once (frame): a line is only remembered at its second placement, so as to
    /// copy nothing for a text that changes every frame.
    text_seen: HashMap<u64, u64>,
    /// Lines copied from memory (counter of the proofs and the bench).
    pub text_hits: u64,
    /// Text drawn and measured without "suspect" glyph (chat input field).
    pub filter_fishy: std::cell::Cell<bool>,
    /// Clip rectangle `[x0, y0, x1, y1]` of the following quads (lists, scrolling texts).
    clip: Option<[f32; 4]>,
    /// "Interface scale" option: 0 = automatic (the largest possible).
    pub scale_option: u32,
    /// Menu background blur: first layer drawn after the blur, and radius (real pixels).
    blur: Option<(usize, i32)>,
    /// First vertex drawn after the blur (all the vertices without blur).
    blur_split: u32,
    /// Layer before which the camera overlays are interleaved (`overlays.rs`), and the
    /// corresponding vertex after the upload.
    overlay_layer: Option<usize>,
    overlay_vertex: u32,
    /// Layer before which the interface's 3D models are interleaved (`special.rs`).
    model_layer: Option<usize>,
    model_vertex: u32,
    /// First tooltip layer of the frame: the IME composition is interleaved before it.
    tooltip_layer: Option<usize>,
    /// First layer after the veil of an in-game screen: the subtitles are interleaved before it.
    veil_layer: Option<usize>,
}

impl Gui {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, assets: GuiAssets) -> Gui {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("interface"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gui.wgsl").into()),
        });
        let img = &assets.atlas.image;
        let sprites = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("interface sprites"),
                size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                // Copy: slots rewritten at run time (server icons).
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &img.rgba,
        );
        let glyph_tex = glyph_texture(device, 1);
        let screen = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("interface"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler =
            device.create_sampler(&wgpu::SamplerDescriptor { label: Some("interface"), ..Default::default() });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let vig = assets.vignette.clone().unwrap_or_else(|| rc_assets::texture::Image::missing(16));
        let vignette = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("vignette"),
                size: wgpu::Extent3d { width: vig.width, height: vig.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &vig.rgba,
        );
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vignette"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let parts = BindParts {
            layout: layout.clone(),
            sprites: sprites.create_view(&Default::default()),
            sampler,
            vignette: vignette.create_view(&Default::default()),
            linear,
        };
        let bind = parts.bind(device, &screen, &glyph_tex);
        let item_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("item atlas (interface)"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let item_bind = (Self::item_bind_group(device, &item_layout, None), u32::MAX);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout), Some(&item_layout)],
            ..Default::default()
        });
        let opts = GuiOpts::from_env();
        let pipelines = Self::make_pipelines(device, &shader, &pl, format, false);
        let inst_pipelines = opts.instance.then(|| Self::make_pipelines(device, &shader, &pl, format, true));
        let atlas_size = (img.width as f32, img.height as f32);
        let special = assets.special;
        let items = assets.items.map(|m| crate::item_atlas::ItemAtlas::new(device, queue, m, special));
        let objects = text_objects::TextObjects::new(&assets.atlas);
        let (aw, ah) = atlas_size;
        let rect = |n: &str| {
            assets
                .atlas
                .sprites
                .get(n)
                .map_or([0.0; 4], |s| [s.x as f32 / aw, s.y as f32 / ah, s.width as f32 / aw, s.height as f32 / ah])
        };
        let mut screen_rects = [0.0; 12];
        screen_rects[..4].copy_from_slice(&rect(END_SKY));
        screen_rects[4..8].copy_from_slice(&rect(END_PORTAL));
        screen_rects[8..].copy_from_slice(&rect(SMOOTH_SPRITE));
        let pre = pre_names().iter().map(|n| assets.atlas.sprites.get(n.as_str()).copied()).collect();
        let smooth = assets.atlas.sprites.get(SMOOTH_SPRITE).copied();
        Gui {
            staging: Vec::new(),
            sent: Vec::new(),
            corners: if opts.format() == Format::Tri { TRI } else { QUAD },
            opts,
            inst_pipelines,
            pipe_src: (shader, pl, format),
            ib: None,
            screen_sent: None,
            screen_rects,
            pre,
            smooth,
            objects,
            pipelines,
            device: device.clone(),
            items,
            item_layout,
            item_bind,
            ranges: Vec::new(),
            bind,
            parts,
            screen,
            glyph_tex,
            glyph_gen: 0,
            vb: None,
            // Pixels sent to the GPU above: only the sprite places remain useful.
            atlas: rc_assets::gui::GuiAtlas {
                image: rc_assets::texture::Image { width: 0, height: 0, rgba: Vec::new() },
                ..assets.atlas
            },
            sprites_tex: sprites,
            atlas_size,
            default_font: default_font(&assets.fonts),
            fonts: assets.fonts,
            font_gen: 0,
            lang: assets.lang,
            white: Glyph::white(),
            missing: Glyph::missing(),
            slots: HashMap::new(),
            pages: Vec::new(),
            layers: vec![(Blend::Normal, Vec::new(), Vec::new())],
            spare: Vec::new(),
            scale: 1,
            pose: 1.0,
            width: 1.0,
            height: 1.0,
            proj: [1.0, 1.0],
            frame_vertices: 0,
            obfuscate_seed: 0x2545_f491,
            bidi_cache: HashMap::new(),
            text_memo: HashMap::new(),
            text_seen: HashMap::new(),
            text_hits: 0,
            filter_fishy: std::cell::Cell::new(false),
            clip: None,
            scale_option: 0,
            blur: None,
            blur_split: 0,
            overlay_layer: None,
            overlay_vertex: 0,
            model_layer: None,
            model_vertex: 0,
            tooltip_layer: None,
            veil_layer: None,
        }
    }

    /// Pipelines of the 7 blends, for vertices (`instanced` false) or instances.
    fn make_pipelines(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        pl: &wgpu::PipelineLayout,
        format: wgpu::TextureFormat,
        instanced: bool,
    ) -> [wgpu::RenderPipeline; 7] {
        // Blends: translucent (src·αs + dst·(1−αs)), inversion (src·(1−dst) + dst·(1−src), alpha
        // of the source), vignette (dst·(1−src), alpha unchanged); no depth.
        let comp = |s, d| wgpu::BlendComponent { src_factor: s, dst_factor: d, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor as F;
        let blends = [
            wgpu::BlendState::ALPHA_BLENDING,
            wgpu::BlendState { color: comp(F::OneMinusDst, F::OneMinusSrc), alpha: comp(F::One, F::Zero) },
            wgpu::BlendState { color: comp(F::Zero, F::OneMinusSrc), alpha: comp(F::Zero, F::One) },
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            wgpu::BlendState { color: comp(F::One, F::One), alpha: comp(F::One, F::One) },
            wgpu::BlendState { color: comp(F::SrcAlpha, F::One), alpha: comp(F::SrcAlpha, F::One) },
            wgpu::BlendState::REPLACE,
        ];
        let vertex_attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Uint32x2];
        let inst_attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32x2
        ];
        let layout = if instanced {
            wgpu::VertexBufferLayout {
                array_stride: size_of::<Inst>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &inst_attrs,
            }
        } else {
            wgpu::VertexBufferLayout {
                array_stride: size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex_attrs,
            }
        };
        blends.map(|blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("interface"),
                layout: Some(pl),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(if instanced { "vs_inst" } else { "vs" }),
                    compilation_options: Default::default(),
                    buffers: &[Some(layout.clone())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
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
        })
    }

    /// Changes the switches (tests and benches): to call between two frames, layers empty.
    #[cfg(test)]
    pub fn set_opts(&mut self, opts: GuiOpts) {
        if opts.instance && self.inst_pipelines.is_none() {
            let (shader, pl, format) = &self.pipe_src;
            self.inst_pipelines = Some(Self::make_pipelines(&self.device, shader, pl, *format, true));
        }
        self.opts = opts;
        self.corners = if opts.format() == Format::Tri { TRI } else { QUAD };
        self.vb = None;
        self.sent.clear();
        self.screen_sent = None;
        self.text_memo.clear();
        self.text_seen.clear();
    }

    /// Start of frame: scale from the window (option, bounded to the maximum), layers emptied.
    pub fn begin(&mut self, window_w: u32, window_h: u32) {
        self.scale = calculate_scale(self.scale_option, window_w, window_h);
        self.clip = None;
        self.proj = projection_size(window_w, window_h, self.scale);
        self.width = self.proj[0].ceil();
        self.height = self.proj[1].ceil();
        for (_, mut shapes, mut text) in self.layers.drain(..) {
            shapes.clear();
            text.clear();
            self.spare.push((shapes, text));
        }
        self.push_layer(Blend::Normal);
        self.pose = 1.0;
        self.blur = None;
        self.overlay_layer = None;
        self.model_layer = None;
        self.tooltip_layer = None;
        self.veil_layer = None;
        self.objects.frame += 1;
        // Reordered lines not drawn for 64 frames: forgotten.
        if self.objects.frame.is_multiple_of(64) {
            let f = self.objects.frame;
            self.bidi_cache.retain(|_, e| f - e.2 < 64);
            self.text_memo.retain(|_, e| f - e.frame < 64);
        }
        // Fingerprints seen: only the previous frame counts.
        if self.text_seen.len() > 4096 {
            let f = self.objects.frame;
            self.text_seen.retain(|_, &mut seen| f - seen < 2);
        }
        self.begin_items();
    }

    /// New empty layer, on a buffer reused from the previous frame if possible.
    fn push_layer(&mut self, blend: Blend) {
        let (shapes, text) = self.spare.pop().unwrap_or_default();
        self.layers.push((blend, shapes, text));
    }

    /// Place of the interface's 3D models (mount entity, book…): everything that follows goes
    /// above them. One place per frame (the last requested).
    pub fn models_here(&mut self) {
        self.model_layer = Some(self.layers.len());
        self.stratum(Blend::Normal);
    }

    /// 3D models are placed in the frame.
    pub fn has_models(&self) -> bool {
        self.model_layer.is_some()
    }

    /// Place of the camera overlays (drawn outside the interface): everything that follows goes
    /// above them.
    pub fn overlays_here(&mut self) {
        self.overlay_layer = Some(self.layers.len());
        self.stratum(Blend::Normal);
    }

    /// Menu background blur (`option-menus.md` § 1.12): everything before (world, HUD or
    /// panorama) is blurred by `radius` real pixels before what follows; < 1: nothing. A single blur
    /// per frame (the first requested).
    pub fn blur(&mut self, radius: i32) {
        if radius < 1 || self.blur.is_some() {
            return;
        }
        let blend = self.layers.last().map_or(Blend::Normal, |l| l.0);
        self.blur = Some((self.layers.len(), radius));
        self.push_layer(blend);
    }

    /// Blur radius requested for the frame.
    pub fn blur_radius(&self) -> Option<i32> {
        self.blur.map(|b| b.1)
    }

    /// Rewrites a sprite of the atlas (same size), e.g. `dyn/server_icon/3`; false if unknown.
    pub fn write_sprite(&self, queue: &wgpu::Queue, name: &str, (w, h): (u32, u32), rgba: &[u8]) -> bool {
        let Some(s) = self.atlas.sprites.get(name) else { return false };
        if (s.width, s.height) != (w, h) || rgba.len() != (w * h * 4) as usize {
            return false;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.sprites_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: s.x, y: s.y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        true
    }

    /// Number of vertices of the current layer (shapes, text), for `transform_since`.
    pub fn mark(&self) -> (usize, usize) {
        self.layers.last().map_or((0, 0), |l| (l.1.len(), l.2.len()))
    }

    /// Affine transformation of the vertices of the current layer added since `mark`:
    /// `x' = a·x + c·y + e`, `y' = b·x + d·y + f` (interface units; rotated splash text).
    pub fn transform_since(&mut self, mark: (usize, usize), [a, b, c, d, e, f]: [f32; 6]) {
        let k = self.pose;
        let Some(l) = self.layers.last_mut() else { return };
        let (shapes, text) = (&mut l.1, &mut l.2);
        for v in shapes.iter_mut().skip(mark.0).chain(text.iter_mut().skip(mark.1)) {
            let [x, y] = v.pos;
            v.pos = [a * x + c * y + e * k, b * x + d * y + f * k];
        }
    }

    /// New stratum above everything, with its blend.
    pub fn stratum(&mut self, blend: Blend) {
        self.push_layer(blend);
    }

    /// Full-screen vignette (`hud.md` § 5.3), colour `(b, b, b)`.
    /// Vignette of colour `c` (grey, or red tint near the world border).
    pub fn vignette_rgb(&mut self, c: [f32; 3]) {
        self.stratum(Blend::Vignette);
        let (w, h) = (self.width, self.height);
        let uv = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        self.quad(false, Self::rect_quad(0.0, 0.0, w, h), uv, [c[0], c[1], c[2], 1.0], [3, 0]);
        self.stratum(Blend::Normal);
    }

    /// Highlight of the selected text, above the text already placed: white inversion
    /// (if `invert`) then addition of pure blue; white text on a dark background comes out blue on white.
    pub fn text_highlight(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, invert: bool) {
        if invert {
            self.stratum(Blend::Invert);
            self.fill(x0, y0, x1, y1, 0xFFFF_FFFF);
        }
        self.stratum(Blend::Additive);
        self.fill(x0, y0, x1, y1, 0xFF00_00FF);
        self.stratum(Blend::Normal);
    }

    /// New layer: what follows goes above everything that precedes (text included).
    #[allow(dead_code)]
    pub fn next_layer(&mut self) {
        let blend = self.layers.last().map_or(Blend::Normal, |l| l.0);
        self.push_layer(blend);
    }

    /// New layer for a tooltip; the first of the frame is kept.
    pub fn tooltip_layer(&mut self) {
        self.tooltip_layer.get_or_insert(self.layers.len());
        self.next_layer();
    }

    /// Draw `f` interleaved just before the first tooltip of the frame (above everything if
    /// there is none), like the game which places the IME composition before its deferred tooltips.
    pub fn draw_before_tooltips(&mut self, f: impl FnOnce(&mut Self)) {
        self.draw_before(self.tooltip_layer, f);
    }

    /// Veil of an "in-game" screen placed: what follows goes into a new layer, and
    /// `draw_after_veil` is interleaved here (the first veil of the frame).
    pub fn veil_here(&mut self) {
        self.veil_layer.get_or_insert(self.layers.len());
        self.next_layer();
    }

    /// Draw `f` interleaved just after the veil of the in-game screen, under its panel (above
    /// everything without a veil in the frame), like the subtitles of the background of these screens in the game.
    pub fn draw_after_veil(&mut self, f: impl FnOnce(&mut Self)) {
        self.draw_before(self.veil_layer, f);
    }

    /// Draw `f` into new layers placed before layer `at` (on top without a mark).
    fn draw_before(&mut self, at: Option<usize>, f: impl FnOnce(&mut Self)) {
        let Some(at) = at else { return f(self) };
        let start = self.layers.len();
        self.push_layer(Blend::Normal);
        f(self);
        let added = self.layers.len() - start;
        let blur = self.blur.as_mut().map(|b| &mut b.0);
        let marks = [
            self.overlay_layer.as_mut(),
            self.model_layer.as_mut(),
            blur,
            self.tooltip_layer.as_mut(),
            self.veil_layer.as_mut(),
        ];
        move_back(&mut self.layers, at, added, marks.into_iter().flatten());
    }

    fn quad(&mut self, text: bool, p: [[f32; 2]; 4], uv: [[f32; 2]; 4], color: [f32; 4], mode: [u32; 2]) {
        self.quad_colors(text, p, uv, [color; 4], mode);
    }

    /// Clipping of the following quads (`None`: none).
    pub fn set_clip(&mut self, r: Option<[f32; 4]>) {
        self.clip = r;
    }

    pub fn clip(&self) -> Option<[f32; 4]> {
        self.clip
    }

    fn quad_colors(&mut self, text: bool, p: [[f32; 2]; 4], uv: [[f32; 2]; 4], colors: [[f32; 4]; 4], mode: [u32; 2]) {
        let (mut p, mut uv, mut colors) = (p, uv, colors);
        if let Some(c) = self.clip {
            // Quad (x0, y0) (x0, y1) (x1, y1) (x1, y0): exact clipping, interpolated coordinates and
            // colours; slanted quad (italic): vertical clipping, horizontal rejection at the centre.
            let (x0, x1) = (p[0][0].min(p[1][0]), p[2][0].max(p[3][0]));
            let (y0, y1) = (p[0][1], p[1][1]);
            let cx = (x0 + x1) / 2.0;
            let skewed = p[0][0] != p[1][0];
            if y1 <= c[1] || y0 >= c[3] || x1 <= c[0] || x0 >= c[2] || (skewed && (cx < c[0] || cx >= c[2])) {
                return;
            }
            let fy = |y: f32| if y1 > y0 { (y - y0) / (y1 - y0) } else { 0.0 };
            let (ty0, ty1) = (fy(y0.max(c[1])), fy(y1.min(c[3])));
            let (tx0, tx1) = if skewed {
                (0.0, 1.0)
            } else {
                let fx = |x: f32| if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
                (fx(x0.max(c[0])), fx(x1.min(c[2])))
            };
            if (tx0, tx1, ty0, ty1) != (0.0, 1.0, 0.0, 1.0) {
                // Bilinear interpolation on the corners (top-left, bottom-left, bottom-right, top-right).
                fn bil<const N: usize>(v: &[[f32; N]; 4], tx: f32, ty: f32) -> [f32; N] {
                    std::array::from_fn(|i| {
                        let top = v[0][i] + (v[3][i] - v[0][i]) * tx;
                        let bottom = v[1][i] + (v[2][i] - v[1][i]) * tx;
                        top + (bottom - top) * ty
                    })
                }
                let at = [(tx0, ty0), (tx0, ty1), (tx1, ty1), (tx1, ty0)];
                let (op, ouv, oc) = (p, uv, colors);
                for (i, (tx, ty)) in at.into_iter().enumerate() {
                    p[i] = bil(&op, tx, ty);
                    uv[i] = bil(&ouv, tx, ty);
                    colors[i] = bil(&oc, tx, ty);
                }
            }
        }
        let (k, corners) = (self.pose, self.corners);
        let layer = self.layers.last_mut().expect("layer");
        let out = if text { &mut layer.2 } else { &mut layer.1 };
        out.extend(corners.iter().map(|&i| Vertex {
            pos: [p[i][0] * k, p[i][1] * k],
            uv: uv[i],
            color: colors[i],
            mode,
        }));
    }

    /// Vertical gradient: `top` at the top, `bottom` at the bottom (ARGB).
    pub fn fill_gradient(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, top: u32, bottom: u32) {
        let (t, b) = (argb(top), argb(bottom));
        self.quad_colors(false, Self::rect_quad(x0, y0, x1, y1), [[0.0; 2]; 4], [t, b, b, t], [2, 0]);
    }

    fn rect_quad(x0: f32, y0: f32, x1: f32, y1: f32) -> [[f32; 2]; 4] {
        [[x0, y0], [x0, y1], [x1, y1], [x1, y0]]
    }

    /// Filled rectangle.
    #[allow(dead_code)]
    pub fn fill(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        self.quad(false, Self::rect_quad(x0, y0, x1, y1), [[0.0; 2]; 4], argb(color), [2, 0]);
    }

    /// Arbitrary filled quadrilateral (vertices in order, no clipping); triangle: p[3] = p[2].
    /// Profiler pie chart (F3+1, `debug_charts.rs`).
    pub fn fill_quad(&mut self, p: [[f32; 2]; 4], color: u32) {
        let (k, c, corners) = (self.pose, argb(color), self.corners);
        let layer = self.layers.last_mut().expect("layer");
        for &i in corners {
            layer.1.push(Vertex { pos: [p[i][0] * k, p[i][1] * k], uv: [0.0; 2], color: c, mode: [2, 0] });
        }
    }

    /// End portal background (loading screen to or from the End): sixteen layers of the
    /// portal texture over the End sky, in screen coordinates, frozen (zero game time).
    pub fn end_portal(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let uv = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        self.quad(false, Self::rect_quad(x0, y0, x1, y1), uv, [1.0; 4], [5, 0]);
    }

    fn sprite_uv(&self, s: &Sprite, u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
        let (aw, ah) = self.atlas_size;
        let x = |f: f32| (s.x as f32 + f * s.width as f32) / aw;
        let y = |f: f32| (s.y as f32 + f * s.height as f32) / ah;
        [[x(u0), y(v0)], [x(u0), y(v1)], [x(u1), y(v1)], [x(u1), y(v0)]]
    }

    /// Part `[u0, u1] × [v0, v1]` (fractions) of the sprite in the given rectangle.
    fn sprite_part(&mut self, s: &Sprite, r: [f32; 4], f: [f32; 4], color: [f32; 4]) {
        let uv = self.sprite_uv(s, f[0], f[1], f[2], f[3]);
        self.quad(false, Self::rect_quad(r[0], r[1], r[2], r[3]), uv, color, [0, 0]);
    }

    /// Rectangle repeated in tiles of `tw × th` GUI pixels (last tile cropped).
    fn tile(&mut self, s: &Sprite, r: [f32; 4], tw: f32, th: f32, f: [f32; 4], color: [f32; 4]) {
        let mut y = r[1];
        while y < r[3] {
            let h = (r[3] - y).min(th);
            let mut x = r[0];
            while x < r[2] {
                let w = (r[2] - x).min(tw);
                let (fu, fv) = ((f[2] - f[0]) * w / tw, (f[3] - f[1]) * h / th);
                self.sprite_part(s, [x, y, x + w, y + h], [f[0], f[1], f[0] + fu, f[1] + fv], color);
                x += tw;
            }
            y += th;
        }
    }

    /// Named sprite (e.g. `hud/crosshair`) in rectangle `(x, y, w, h)`, according to its scaling
    /// mode; unknown sprite: nothing.
    pub fn sprite(&mut self, name: &str, x: f32, y: f32, w: f32, h: f32, color: u32) {
        let s = self.atlas.sprites.get(name).copied();
        self.sprite_of(s, x, y, w, h, color);
    }

    /// Frequent sprite [`Pre`]: resolved at creation (`RUSTCRAFT_OPT_SPRITEIDS`, otherwise looked up by
    /// name like [`Gui::sprite`]); absent from the atlas: `None`.
    pub fn pre(&self, id: Pre) -> Option<Sprite> {
        if self.opts.sprite_ids {
            self.pre[id.0 as usize]
        } else {
            self.atlas.sprites.get(pre_names()[id.0 as usize].as_str()).copied()
        }
    }

    /// Like [`Gui::sprite`] for a frequent sprite [`Pre`].
    pub fn sprite_pre(&mut self, id: Pre, x: f32, y: f32, w: f32, h: f32, color: u32) {
        let s = self.pre(id);
        self.sprite_of(s, x, y, w, h, color);
    }

    /// Like [`Gui::sprite_region`] for a frequent sprite [`Pre`].
    #[allow(clippy::too_many_arguments)]
    pub fn sprite_region_pre(&mut self, id: Pre, sw: f32, sh: f32, u: f32, v: f32, x: f32, y: f32, w: f32, h: f32) {
        let Some(s) = self.pre(id) else { return };
        self.sprite_part(&s, [x, y, x + w, y + h], [u / sw, v / sh, (u + w) / sw, (v + h) / sh], argb(0xffff_ffff));
    }

    /// Body of [`Gui::sprite`] for an already looked-up sprite.
    fn sprite_of(&mut self, s: Option<Sprite>, x: f32, y: f32, w: f32, h: f32, color: u32) {
        let Some(s) = s else { return };
        let c = argb(color);
        match s.scaling {
            Scaling::Stretch => self.sprite_part(&s, [x, y, x + w, y + h], [0.0, 0.0, 1.0, 1.0], c),
            Scaling::Tile { width, height } => {
                self.tile(&s, [x, y, x + w, y + h], width as f32, height as f32, [0.0, 0.0, 1.0, 1.0], c)
            }
            Scaling::NineSlice { width, height, border, stretch_inner } => {
                let (sw, sh) = (width as f32, height as f32);
                if w == sw && h == sh {
                    return self.sprite_part(&s, [x, y, x + w, y + h], [0.0, 0.0, 1.0, 1.0], c);
                }
                let l = (border[0] as f32).min(w / 2.0);
                let t = (border[1] as f32).min(h / 2.0);
                let r = (border[2] as f32).min(w / 2.0);
                let b = (border[3] as f32).min(h / 2.0);
                let xs = [x, x + l, x + w - r, x + w];
                let ys = [y, y + t, y + h - b, y + h];
                let fu = [0.0, l / sw, 1.0 - r / sw, 1.0];
                let fv = [0.0, t / sh, 1.0 - b / sh, 1.0];
                let (iw, ih) = (sw - l - r, sh - t - b);
                for j in 0..3 {
                    for i in 0..3 {
                        let rect = [xs[i], ys[j], xs[i + 1], ys[j + 1]];
                        if rect[2] <= rect[0] || rect[3] <= rect[1] {
                            continue;
                        }
                        let fr = [fu[i], fv[j], fu[i + 1], fv[j + 1]];
                        let corner = i != 1 && j != 1;
                        if corner || stretch_inner {
                            self.sprite_part(&s, rect, fr, c);
                        } else {
                            let tw = if i == 1 { iw } else { rect[2] - rect[0] };
                            let th = if j == 1 { ih } else { rect[3] - rect[1] };
                            self.tile(&s, rect, tw.max(1.0), th.max(1.0), fr, c);
                        }
                    }
                }
            }
        }
    }

    /// Texture repeated in tiles of `tw × th` units over `[x0, x1] × [y0, y1]`, pattern offset by
    /// `(u, v)` units (menu and list backgrounds that follow the scrolling).
    #[allow(clippy::too_many_arguments)]
    pub fn sprite_tiled(&mut self, name: &str, x0: f32, y0: f32, x1: f32, y1: f32, tw: f32, th: f32, u: f32, v: f32) {
        let Some(&s) = self.atlas.sprites.get(name) else { return };
        let old = self.clip;
        let r = [x0, y0, x1, y1];
        let c = match old {
            Some(c) => [c[0].max(r[0]), c[1].max(r[1]), c[2].min(r[2]), c[3].min(r[3])],
            None => r,
        };
        let (sx, sy) = (x0 - u.rem_euclid(tw), y0 - v.rem_euclid(th));
        if self.opts.tile_shader {
            // A single quad on the clipped area; uv in tiles from the pattern origin, atlas rectangle
            // in the colour: the shader repeats the texture (mode 7), like the game
            // which draws this background as one quad with a repeated texture.
            if c[2] <= c[0] || c[3] <= c[1] {
                return;
            }
            let (aw, ah) = self.atlas_size;
            let rect = [s.x as f32 / aw, s.y as f32 / ah, s.width as f32 / aw, s.height as f32 / ah];
            let (u0, v0, u1, v1) = ((c[0] - sx) / tw, (c[1] - sy) / th, (c[2] - sx) / tw, (c[3] - sy) / th);
            self.clip = None;
            self.quad(
                false,
                Self::rect_quad(c[0], c[1], c[2], c[3]),
                [[u0, v0], [u0, v1], [u1, v1], [u1, v0]],
                rect,
                [7, 0],
            );
            self.clip = old;
            return;
        }
        self.clip = Some(c);
        self.tile(&s, [sx, sy, x1, y1], tw, th, [0.0, 0.0, 1.0, 1.0], [1.0; 4]);
        self.clip = old;
    }

    /// Portion `(u, v, w, h)` (nominal pixels) of a stretched sprite of nominal size `(sw, sh)`.
    #[allow(clippy::too_many_arguments)]
    pub fn sprite_region(&mut self, name: &str, sw: f32, sh: f32, u: f32, v: f32, x: f32, y: f32, w: f32, h: f32) {
        self.sprite_region_color(name, (sw, sh), (u, v), [x, y, w, h], 0xffff_ffff);
    }

    /// Portion of a sprite of nominal size `(sw, sh)` from `(u, v)`, tinted (ARGB).
    /// Part `src = [u, v, w, h]` (pixels of a sprite of `size`) stretched into `dst`, tinted.
    pub fn sprite_stretch(&mut self, name: &str, size: (f32, f32), src: [f32; 4], dst: [f32; 4], color: u32) {
        let Some(&s) = self.atlas.sprites.get(name) else { return };
        let [u, v, w, h] = src;
        let [x, y, dw, dh] = dst;
        let uv = [u / size.0, v / size.1, (u + w) / size.0, (v + h) / size.1];
        self.sprite_part(&s, [x, y, x + dw, y + dh], uv, argb(color));
    }

    pub fn sprite_region_color(
        &mut self,
        name: &str,
        (sw, sh): (f32, f32),
        (u, v): (f32, f32),
        r: [f32; 4],
        color: u32,
    ) {
        let Some(&s) = self.atlas.sprites.get(name) else { return };
        let [x, y, w, h] = r;
        self.sprite_part(&s, [x, y, x + w, y + h], [u / sw, v / sh, (u + w) / sw, (v + h) / sh], argb(color));
    }

    /// Fraction `uv` (`[u0, v0, u1, v1]`) of the loading logo stretched over `rect` (`[x0, y0, x1, y1]`),
    /// smoothed (linear filter, clamped edges) like in the game.
    pub fn sprite_smooth(&mut self, rect: [f32; 4], uv: [f32; 4], color: u32) {
        let s = if self.opts.sprite_ids { self.smooth } else { self.atlas.sprites.get(SMOOTH_SPRITE).copied() };
        let Some(s) = s else { return };
        let uv = self.sprite_uv(&s, uv[0], uv[1], uv[2], uv[3]);
        self.quad(false, Self::rect_quad(rect[0], rect[1], rect[2], rect[3]), uv, argb(color), [6, 0]);
    }

    /// Replaces the fonts ("forced Unicode", "Japanese variants" options): the glyphs
    /// already sent stay under the old key, the new ones take a fresh place.
    pub fn set_fonts(&mut self, fonts: Vec<(String, Font)>) {
        self.fonts = fonts;
        self.default_font = default_font(&self.fonts);
        self.font_gen += 1;
        self.glyph_gen += 1;
    }

    fn font_index(&self, font: &str) -> Option<usize> {
        if font.is_empty() && self.opts.font_table {
            return self.default_font;
        }
        let id = if font.is_empty() { "minecraft:default" } else { font };
        self.fonts.iter().position(|f| f.0 == id)
    }

    fn glyph(&self, font: Option<usize>, c: char) -> Glyph {
        let f = font.map(|f| &self.fonts[f].1);
        let g = if self.filter_fishy.get() {
            f.and_then(|f| f.glyph_non_fishy(c as u32))
        } else {
            f.and_then(|f| f.glyph(c as u32))
        };
        g.unwrap_or_else(|| self.missing.clone())
    }

    /// Like [`Gui::glyph`], lent by the font's flat table (`RUSTCRAFT_OPT_FONTTABLE`).
    fn glyph_ref(&self, font: Option<usize>, c: char) -> std::borrow::Cow<'_, Glyph> {
        let f = font.map(|f| &self.fonts[f].1);
        let g = if self.filter_fishy.get() {
            f.and_then(|f| f.glyph_non_fishy_ref(c as u32))
        } else {
            f.and_then(|f| f.glyph_ref(c as u32))
        };
        g.unwrap_or(std::borrow::Cow::Borrowed(&self.missing))
    }

    /// Place key in the pages: the replacement glyph of a suspect glyph has its own.
    fn glyph_key(&self, font: Option<usize>, c: u32, fishy_free: bool) -> (usize, u32) {
        let alt = fishy_free
            && font.is_some_and(|f| {
                let f = &self.fonts[f].1;
                if self.opts.font_table { f.first_is_fishy_ref(c) } else { f.first_is_fishy(c) }
            });
        (font.map_or(usize::MAX, |f| f + self.font_gen * 64), c | u32::from(alt) << 31)
    }

    /// Doubled glyph atlas (pages copied on the GPU): it only reserves the pages
    /// in use (one at the start) instead of the 32 possible.
    fn grow_glyphs(&mut self, queue: &wgpu::Queue) {
        let old = self.glyph_tex.depth_or_array_layers();
        let tex = glyph_texture(&self.device, (old * 2).min(MAX_PAGES));
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("glyphs") });
        enc.copy_texture_to_texture(
            self.glyph_tex.as_image_copy(),
            tex.as_image_copy(),
            wgpu::Extent3d { width: PAGE, height: PAGE, depth_or_array_layers: old },
        );
        queue.submit([enc.finish()]);
        self.bind = self.parts.bind(&self.device, &self.screen, &tex);
        self.glyph_tex = tex;
        self.glyph_gen += 1;
    }

    /// Place of a glyph in the pages (sent on first use).
    fn slot(&mut self, queue: &wgpu::Queue, key: (usize, u32), g: &Glyph) -> Option<Slot> {
        if let Some(s) = self.slots.get(&key) {
            return Some(*s);
        }
        let bitmap = g.bitmap.as_ref()?;
        let (w, h) = bitmap.size();
        let fits = |p: &(u32, u32, u32)| (p.0 + w <= PAGE && p.1 + h <= PAGE) || p.1 + p.2 + 1 + h <= PAGE;
        let page = match self.pages.iter().position(fits) {
            Some(i) => i,
            None if (self.pages.len() as u32) < MAX_PAGES => {
                if self.pages.len() as u32 == self.glyph_tex.depth_or_array_layers() {
                    self.grow_glyphs(queue);
                }
                self.pages.push((0, 0, 0));
                self.pages.len() - 1
            }
            None => return None,
        };
        let p = &mut self.pages[page];
        if p.0 + w > PAGE {
            p.0 = 0;
            p.1 += p.2 + 1;
            p.2 = 0;
        }
        let slot = Slot { page: page as u32, x: p.0, y: p.1, w, h };
        p.0 += w + 1;
        p.2 = p.2.max(h);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.glyph_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: slot.x, y: slot.y, z: slot.page },
                aspect: wgpu::TextureAspect::All,
            },
            &bitmap.rgba(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.slots.insert(key, slot);
        Some(slot)
    }

    /// Quad of a glyph placed in the pages.
    #[allow(clippy::too_many_arguments)]
    fn glyph_quad_at(&mut self, s: Slot, g: Geo, x: f32, y: f32, italic: bool, bold_grow: bool, color: [f32; 4]) {
        let e = if bold_grow { 0.1 } else { 0.0 };
        let (l, r, t, b) = (x + g.left - e, x + g.right + e, y + g.top - e, y + g.bottom + e);
        let skew = |yy: f32| if italic { 1.0 - 0.25 * yy } else { 0.0 };
        let (st, sb) = (skew(g.top), skew(g.bottom));
        let p = [[l + st, t], [l + sb, b], [r + sb, b], [r + st, t]];
        let u0 = (s.x as f32 + 0.01) / PAGE as f32;
        let u1 = (s.x as f32 - 0.01 + s.w as f32) / PAGE as f32;
        let v0 = (s.y as f32 + 0.01) / PAGE as f32;
        let v1 = (s.y as f32 - 0.01 + s.h as f32) / PAGE as f32;
        let uv = [[u0, v0], [u0, v1], [u1, v1], [u1, v0]];
        self.quad(true, p, uv, color, [1, s.page]);
    }

    /// Glyph drawn at random with the same rounded width (obfuscated text).
    fn obfuscated(&mut self, font: Option<usize>, g: &Glyph) -> (char, Glyph) {
        let target = g.advance.ceil() as u32;
        let Some(f) = font else { return (' ', self.missing.clone()) };
        let len = self.fonts[f].1.with_width(target).len();
        if len == 0 {
            return (' ', self.missing.clone());
        }
        self.obfuscate_seed ^= self.obfuscate_seed << 13;
        self.obfuscate_seed ^= self.obfuscate_seed >> 7;
        self.obfuscate_seed ^= self.obfuscate_seed << 17;
        let font = &self.fonts[f].1;
        let c = font.with_width(target)[(self.obfuscate_seed % len as u64) as usize];
        // The game always takes the non-suspect glyph here.
        let g = font.glyph_non_fishy(c).unwrap_or_else(|| self.missing.clone());
        (char::from_u32(c).unwrap_or(' '), g)
    }

    /// Draws a text line (`text.md` § 5): base ARGB `color` (alpha 0: nothing),
    /// drop shadow, background (alpha 0: none). Returns the final x. The line (logical order)
    /// is placed in visual order, Arabic letters shaped (§ 5.1), like any
    /// component drawn by the game.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        queue: &wgpu::Queue,
        chars: &[StyledChar],
        x: f32,
        y: f32,
        color: u32,
        shadow: bool,
        background: u32,
    ) -> f32 {
        let order = self.visual_order(chars);
        self.text_in(queue, chars, order.as_deref(), [x, y], color, shadow, background)
    }

    /// Like [`Gui::text`], characters in the order received, without shaping: input fields
    /// (the game draws the typed text as is there).
    pub fn text_forward(
        &mut self,
        queue: &wgpu::Queue,
        chars: &[StyledChar],
        x: f32,
        y: f32,
        color: u32,
        shadow: bool,
    ) -> f32 {
        self.text_in(queue, chars, None, [x, y], color, shadow, 0)
    }

    /// Like [`Gui::text`] for a plain string (not a component): the game only reorders it
    /// in a right-to-left language.
    pub fn text_string(
        &mut self,
        queue: &wgpu::Queue,
        chars: &[StyledChar],
        x: f32,
        y: f32,
        color: u32,
        shadow: bool,
    ) -> f32 {
        if self.lang.bidirectional {
            self.text(queue, chars, x, y, color, shadow, 0)
        } else {
            self.text_forward(queue, chars, x, y, color, shadow)
        }
    }

    /// Width of a line as [`Gui::text`] places it (Arabic forms included).
    pub fn visual_width(&self, chars: &[StyledChar]) -> i32 {
        let Some(order) = self.visual_order_of(chars) else { return rc_text::width(self, chars) };
        let m: &dyn rc_text::Metrics = self;
        order
            .iter()
            .map(|&(c, k)| {
                let st = &chars[k as usize].style;
                let (a, bold) = m.advance(&st.font, c);
                if st.bold { a + bold } else { a }
            })
            .sum::<f32>()
            .ceil() as i32
    }

    /// Visual order of a line (`None`: as is) without touching the cache: the cache's own
    /// if it is there, otherwise computed. To measure or search under the mouse what `text` places.
    pub fn visual_order_of(&self, chars: &[StyledChar]) -> Option<std::sync::Arc<[(char, u32)]>> {
        let rtl = self.lang.bidirectional;
        if rc_text::bidi::unchanged(chars, rtl) {
            return None;
        }
        if !rc_data::env_flag!("RUSTCRAFT_OPT_NOBIDICACHE")
            && let Some(e) = self.bidi_cache.get(&self.bidi_key(chars, rtl)).filter(|e| bidi_entry_is(e, chars, rtl))
        {
            return Some(e.1.clone());
        }
        rc_text::bidi::visual(chars, rtl).map(Into::into)
    }

    fn bidi_key(&self, chars: &[StyledChar], rtl: bool) -> u64 {
        use std::hash::{BuildHasher, Hasher};
        let mut h = self.bidi_cache.hasher().build_hasher();
        for s in chars {
            h.write_u32(s.c as u32);
        }
        h.write_u8(u8::from(rtl));
        h.finish()
    }

    /// Visual order of a line (`None`: as is), kept from one frame to the next for the
    /// lines to reorder (`RUSTCRAFT_OPT_NOBIDICACHE`: recomputed every time).
    fn visual_order(&mut self, chars: &[StyledChar]) -> Option<std::sync::Arc<[(char, u32)]>> {
        let rtl = self.lang.bidirectional;
        if rc_text::bidi::unchanged(chars, rtl) {
            return None;
        }
        if rc_data::env_flag!("RUSTCRAFT_OPT_NOBIDICACHE") {
            return rc_text::bidi::visual(chars, rtl).map(Into::into);
        }
        let frame = self.objects.frame;
        let key = self.bidi_key(chars, rtl);
        if let Some(e) = self.bidi_cache.get_mut(&key)
            && bidi_entry_is(e, chars, rtl)
        {
            e.2 = frame;
            return Some(e.1.clone());
        }
        let order: std::sync::Arc<[(char, u32)]> = rc_text::bidi::visual(chars, rtl)?.into();
        self.bidi_cache.insert(key, ((chars.iter().map(|s| s.c).collect(), rtl), order.clone(), frame));
        Some(order)
    }

    /// `order`: visual order (character, rank of the source character that gives the style);
    /// `None`: `chars` as is. With `RUSTCRAFT_OPT_TEXTCACHE`, a line that comes back
    /// identical (same characters, styles and parameters) copies its remembered vertices; obfuscated
    /// text (random draw at each placement) and objects in the text (atlas places taken
    /// per frame) are always placed again.
    #[allow(clippy::too_many_arguments)]
    fn text_in(
        &mut self,
        queue: &wgpu::Queue,
        chars: &[StyledChar],
        order: Option<&[(char, u32)]>,
        at: [f32; 2],
        color: u32,
        shadow: bool,
        background: u32,
    ) -> f32 {
        if !self.opts.text_cache || color >> 24 == 0 || self.layers.is_empty() {
            return self.text_layout(queue, chars, order, at, color, shadow, background);
        }
        let flags = u8::from(shadow)
            | u8::from(self.filter_fishy.get()) << 1
            | u8::from(order.is_some()) << 2
            | u8::from(self.lang.bidirectional) << 3;
        let params = TextParams {
            xy: at.map(f32::to_bits),
            color,
            background,
            pose: self.pose.to_bits(),
            clip: self.clip.map(|c| c.map(f32::to_bits)),
            corners: self.corners.len(),
            font_gen: self.font_gen,
            flags,
        };
        let key = {
            use std::hash::{BuildHasher, Hash, Hasher};
            let mut h = self.text_memo.hasher().build_hasher();
            params.hash(&mut h);
            for s in chars {
                let st = &s.style;
                // Random draw at each placement, or atlas place taken per frame: never remembered.
                if st.obfuscated && s.c != ' ' || !st.font.is_empty() && rc_text::object_font(&st.font).is_some() {
                    return self.text_layout(queue, chars, order, at, color, shadow, background);
                }
                h.write_u32(s.c as u32);
                st.color.hash(&mut h);
                st.shadow_color.hash(&mut h);
                h.write_u8(
                    u8::from(st.bold)
                        | u8::from(st.italic) << 1
                        | u8::from(st.underlined) << 2
                        | u8::from(st.strikethrough) << 3,
                );
                st.font.hash(&mut h);
            }
            h.finish()
        };
        let frame = self.objects.frame;
        if let Some(m) = self.text_memo.get_mut(&key)
            && m.params == params
            && m.chars.len() == chars.len()
            && m.chars.iter().zip(chars).all(|(a, b)| same_char(a, b))
        {
            m.frame = frame;
            self.text_hits += 1;
            let layer = self.layers.last_mut().expect("layer");
            layer.2.extend_from_slice(&m.verts);
            return m.end;
        }
        let start = self.layers.last().map_or(0, |l| l.2.len());
        let end = self.text_layout(queue, chars, order, at, color, shadow, background);
        // Remembered only at its second placement (a text that changes every frame copies nothing).
        if self.text_seen.insert(key, frame).is_some() && self.text_memo.len() < 4096 {
            let verts = self.layers.last().map_or(&[][..], |l| &l.2[start..]).into();
            self.text_memo.insert(key, TextMemo { chars: chars.into(), params, verts, end, frame });
        }
        end
    }

    /// Placement of a text line, glyph by glyph (see [`Gui::text_in`]).
    #[allow(clippy::too_many_arguments)]
    fn text_layout(
        &mut self,
        queue: &wgpu::Queue,
        chars: &[StyledChar],
        order: Option<&[(char, u32)]>,
        [x, y]: [f32; 2],
        color: u32,
        shadow: bool,
        background: u32,
    ) -> f32 {
        if color >> 24 == 0 {
            return x;
        }
        let base_alpha = color & 0xff00_0000;
        let mut cx = x;
        let mut effects: Vec<Effect> = Vec::new();
        let start_layer_len = self.layers.last().map_or(0, |l| l.2.len());
        let mut prev_style: Option<&rc_text::CharStyle> = None;
        // Font and kind (object or glyph) of the current run of the same style (font table).
        let mut run: (Option<usize>, bool) = (None, false);
        let table = self.opts.font_table;
        let n = order.map_or(chars.len(), <[_]>::len);
        for k in 0..n {
            let (ch, style) = match order {
                Some(o) => (o[k].0, &chars[o[k].1 as usize].style),
                None => (chars[k].c, &chars[k].style),
            };
            let first_of_run = prev_style != Some(style);
            let (font, object) = if table && !first_of_run {
                run
            } else {
                (self.font_index(&style.font), rc_text::object_font(&style.font).is_some())
            };
            run = (font, object);
            // Glyph: metrics and place in the pages. Font table (neither object nor
            // obfuscated): lent glyph, place resolved right away (no object to place before).
            let (geo, place, key);
            if table && !object && !(style.obfuscated && ch != ' ') {
                key = self.glyph_key(font, ch as u32, self.filter_fishy.get());
                let g = self.glyph_ref(font, ch);
                geo = Geo::of(&g);
                place = if g.bitmap.is_none() {
                    Place::NoImage
                } else if let Some(s) = self.slots.get(&key) {
                    Place::At(Some(*s))
                } else {
                    let g = g.into_owned();
                    Place::At(self.slot(queue, key, &g))
                };
            } else {
                let mut g = self.glyph(font, ch);
                let mut kk = self.glyph_key(font, ch as u32, self.filter_fishy.get());
                if style.obfuscated && ch != ' ' {
                    let c;
                    (c, g) = self.obfuscated(font, &g);
                    kk = self.glyph_key(font, c as u32, true);
                }
                key = kk;
                geo = Geo::of(&g);
                // Place resolved at the first quad, after the possible object (order of the old path).
                place = if g.bitmap.is_some() { Place::Later(g) } else { Place::NoImage };
            }
            let rgb = style.color.map_or(color, |col| base_alpha | col);
            let text_color = argb(rgb);
            let shadow_color = match style.shadow_color {
                Some(0) => None,
                Some(sh) => {
                    if text_color[3] >= 1.0 {
                        Some(sh)
                    } else {
                        let a = (text_color[3] * ((sh >> 24) as f32 / 255.0) * 255.0).floor() as u32;
                        Some((sh & 0xff_ffff) | a << 24)
                    }
                }
                None if shadow => {
                    let q = |s: u32| (((rgb >> s) & 0xff) as f32 * 0.25) as u32;
                    Some((rgb & 0xff00_0000) | q(16) << 16 | q(8) << 8 | q(0))
                }
                None => None,
            };
            // Object in the text: 8 × 8 sprite tinted like a glyph, with its shadow, top at
            // 7 − 8 = 1 above the line (y − 1); advance 8, shadow and bold offsets 1.
            // Unknown atlas: ordinary missing glyph.
            let sprite = match object.then(|| rc_text::object_font(&style.font)).flatten() {
                Some(o) => self.object_sprite(queue, &style.font, o),
                None => None,
            };
            let (d, gb) = if sprite.is_some() { (1.0, 1.0) } else { (geo.offset, geo.offset) };
            let a = if sprite.is_some() { 8.0 } else { geo.advance } + if style.bold { gb } else { 0.0 };
            if let Some(s) = sprite {
                if let Some(sh) = shadow_color {
                    self.sprite_part(&s, object_rect(cx + d, y + d), [0.0, 0.0, 1.0, 1.0], argb(sh));
                }
                self.sprite_part(&s, object_rect(cx, y), [0.0, 0.0, 1.0, 1.0], text_color);
            } else {
                let slot = match place {
                    Place::NoImage => None,
                    Place::At(s) => s,
                    Place::Later(g) => self.slot(queue, key, &g),
                };
                if let Some(s) = slot {
                    let it = style.italic;
                    if let Some(sh) = shadow_color {
                        let sc = argb(sh);
                        self.glyph_quad_at(s, geo, cx + d, y + d, it, style.bold, sc);
                        if style.bold {
                            self.glyph_quad_at(s, geo, cx + gb + d, y + d, it, true, sc);
                        }
                    }
                    self.glyph_quad_at(s, geo, cx, y, it, style.bold, text_color);
                    if style.bold {
                        self.glyph_quad_at(s, geo, cx + gb, y, it, true, text_color);
                    }
                }
            }
            let x0 = if first_of_run { cx - 1.0 } else { cx };
            let sh = shadow_color.map(|s| (argb(s), d));
            if style.strikethrough {
                effects.push(([x0, y + 3.5, cx + a, y + 4.5], text_color, sh));
            }
            if style.underlined {
                effects.push(([x0, y + 8.0, cx + a, y + 9.0], text_color, sh));
            }
            prev_style = Some(style);
            cx += a;
        }
        // Background first (inserted before the glyphs of this line), effects after.
        if background >> 24 != 0 {
            let bg = argb(background);
            let q = Self::rect_quad(x - 1.0, y - 1.0, cx, y + 9.0);
            let (k, corners) = (self.pose, self.corners);
            let tri = corners.iter().map(|&i| Vertex {
                pos: [q[i][0] * k, q[i][1] * k],
                uv: [0.0; 2],
                color: bg,
                mode: [2, 0],
            });
            let layer = self.layers.last_mut().expect("layer");
            layer.2.splice(start_layer_len..start_layer_len, tri);
        }
        let white = self.white.clone();
        for (r, col, sh) in effects {
            if let Some((sc, d)) = sh {
                self.effect(queue, &white, [r[0] + d, r[1] + d, r[2] + d, r[3] + d], sc);
            }
            self.effect(queue, &white, r, col);
        }
        cx
    }

    /// Text shaped outside the interface (name tags): triangles in font pixels,
    /// centred in x (x = −width / 2), background included; same styles as `text`.
    pub fn text_quads(
        &mut self,
        queue: &wgpu::Queue,
        chars: &[StyledChar],
        color: u32,
        background: u32,
    ) -> Vec<Vertex> {
        // Triangles of 6 vertices whatever the interface format (read by `nametag::glyphs`).
        let pose = std::mem::replace(&mut self.pose, 1.0);
        let corners = std::mem::replace(&mut self.corners, TRI);
        self.layers.push((Blend::Normal, Vec::new(), Vec::new()));
        let w = self.text(queue, chars, 0.0, 0.0, color, false, background);
        let (_, mut out, mut text) = self.layers.pop().expect("layer pushed above");
        self.pose = pose;
        self.corners = corners;
        out.append(&mut text);
        for v in &mut out {
            v.pos[0] -= w / 2.0;
        }
        out
    }

    /// Like `text_quads`, without centring (x = 0 on the left) and with optional drop shadow
    /// (text displays).
    pub fn text_quads_left(
        &mut self,
        queue: &wgpu::Queue,
        chars: &[StyledChar],
        color: u32,
        shadow: bool,
    ) -> Vec<Vertex> {
        let pose = std::mem::replace(&mut self.pose, 1.0);
        let corners = std::mem::replace(&mut self.corners, TRI);
        self.layers.push((Blend::Normal, Vec::new(), Vec::new()));
        self.text(queue, chars, 0.0, 0.0, color, shadow, 0);
        let (_, mut out, mut text) = self.layers.pop().expect("layer pushed above");
        self.pose = pose;
        self.corners = corners;
        out.append(&mut text);
        out
    }

    /// View of the glyph atlas (page array).
    pub fn glyph_view(&self) -> wgpu::TextureView {
        self.glyph_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }

    fn effect(&mut self, queue: &wgpu::Queue, white: &Glyph, r: [f32; 4], color: [f32; 4]) {
        let Some(s) = self.slot(queue, (usize::MAX - 1, 0), white) else { return };
        let (u, v) = ((s.x as f32 + 1.5) / PAGE as f32, (s.y as f32 + 1.5) / PAGE as f32);
        self.quad(true, Self::rect_quad(r[0], r[1], r[2], r[3]), [[u, v]; 4], color, [1, s.page]);
    }

    fn item_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        let dummy;
        let view = match view {
            Some(v) => v,
            None => {
                dummy = device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("empty item atlas"),
                        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default());
                &dummy
            }
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("item atlas (interface)"),
            layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }],
        })
    }

    /// Item in a slot (`hud.md` § 3.2, 3.4): `key` = render identity, `layers` its
    /// layers; `pop` = remaining pickup animation (ticks, partial included).
    pub fn item(&mut self, key: u64, layers: &[rc_assets::item::Layer], x: f32, y: f32, pop: f32) {
        let Some(atlas) = &mut self.items else { return };
        let Some(uv) = atlas.cell_for(&self.device, key, layers) else { return };
        let mut p = Self::rect_quad(x, y, x + 16.0, y + 16.0);
        if pop > 0.0 {
            // Scale (1/k, (k+1)/2) around (x + 8, y + 12).
            let k = 1.0 + pop / 5.0;
            let (sx, sy) = (1.0 / k, (k + 1.0) / 2.0);
            for c in &mut p {
                c[0] = x + 8.0 + (c[0] - x - 8.0) * sx;
                c[1] = y + 12.0 + (c[1] - y - 12.0) * sy;
            }
        }
        let (u0, v0, u1, v1) = (uv[0], uv[1], uv[0] + uv[2], uv[1] + uv[3]);
        self.quad(false, p, [[u0, v0], [u0, v1], [u1, v1], [u1, v0]], [1.0; 4], [4, 0]);
    }

    /// Item scaled by `s` from corner `(x, y)` (thumbnails, notifications).
    pub fn item_scaled(&mut self, key: u64, layers: &[rc_assets::item::Layer], x: f32, y: f32, s: f32) {
        let Some(atlas) = &mut self.items else { return };
        let Some(uv) = atlas.cell_for(&self.device, key, layers) else { return };
        let p = Self::rect_quad(x, y, x + 16.0 * s, y + 16.0 * s);
        let (u0, v0, u1, v1) = (uv[0], uv[1], uv[0] + uv[2], uv[1] + uv[3]);
        self.quad(false, p, [[u0, v0], [u0, v1], [u1, v1], [u1, v0]], [1.0; 4], [4, 0]);
    }

    /// Draws the pending item cells into the atlas (before the frame pass).
    pub fn encode_items(
        &mut self,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        if let Some(a) = &mut self.items {
            a.encode(&self.device, queue, enc, timer);
        }
    }

    /// Start of the item atlas for the frame (current GUI scale).
    fn begin_items(&mut self) {
        if let Some(a) = &mut self.items {
            a.begin(&self.device, self.scale);
        }
    }

    /// Rectangles of the screen uniform (atlas fractions) looked up by name (old path).
    fn screen_rects_by_name(&self) -> [f32; 12] {
        let (aw, ah) = self.atlas_size;
        let rect = |n: &str| {
            self.atlas
                .sprites
                .get(n)
                .map_or([0.0; 4], |s| [s.x as f32 / aw, s.y as f32 / ah, s.width as f32 / aw, s.height as f32 / ah])
        };
        let mut r = [0.0; 12];
        r[..4].copy_from_slice(&rect(END_SKY));
        r[4..8].copy_from_slice(&rect(END_PORTAL));
        r[8..].copy_from_slice(&rect(SMOOTH_SPRITE));
        r
    }

    /// Bytes sent for `n` placed vertices (6 or 4 per quad depending on the format).
    fn stored_bytes(fmt: Format, n: usize) -> usize {
        match fmt {
            Format::Tri | Format::Indexed => n * size_of::<Vertex>(),
            Format::Instanced => n / 4 * size_of::<Inst>(),
        }
    }

    /// Elements drawn for `n` placed vertices: vertices, indices or instances.
    fn units(fmt: Format, n: usize) -> u32 {
        (match fmt {
            Format::Tri => n,
            Format::Indexed => n / 4 * 6,
            Format::Instanced => n / 4,
        }) as u32
    }

    /// Bytes of the frame in the upload format, in chunks, in order (layers, shapes then
    /// text of each layer).
    fn encode(layers: &[(Blend, Vec<Vertex>, Vec<Vertex>)], fmt: Format, sink: &mut impl FnMut(&[u8])) {
        for (_, shapes, text) in layers {
            for part in [shapes, text] {
                if fmt != Format::Instanced {
                    sink(bytemuck::cast_slice(part));
                    continue;
                }
                // Instances in batches on the stack (no allocation).
                let mut batch = [<Inst as bytemuck::Zeroable>::zeroed(); 64];
                for quads in part.chunks(4 * batch.len()) {
                    let n = quads.len() / 4;
                    for (b, q) in batch.iter_mut().zip(quads.as_chunks::<4>().0) {
                        *b = Inst::of(q);
                    }
                    sink(bytemuck::cast_slice(&batch[..n]));
                }
            }
        }
    }

    /// Sends the frame's quads (layers in order, text after the shapes of each layer).
    /// `up`: shared staging buffer (direct write and upload of changes only).
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, up: &mut crate::render::upload::Uploader) {
        if let Some(a) = &self.items
            && a.generation != self.item_bind.1
        {
            self.item_bind = (Self::item_bind_group(device, &self.item_layout, a.view()), a.generation);
        }
        // Interface size, then rectangles (atlas fractions) of the End sky and portal
        // for the loading background, and of the smoothed sprite.
        let mut u = [0.0f32; 16];
        u[..2].copy_from_slice(&self.proj);
        u[4..].copy_from_slice(&if self.opts.sprite_ids { self.screen_rects } else { self.screen_rects_by_name() });
        if !self.opts.uniform || self.screen_sent != Some(u) {
            queue.write_buffer(&self.screen, 0, bytemuck::cast_slice(&u));
            self.screen_sent = Some(u);
        }
        let fmt = self.opts.format();
        self.ranges.clear();
        self.blur_split = u32::MAX;
        self.overlay_vertex = 0;
        // After the place of the overlays, the next range starts anew (no merging).
        let mut fresh = false;
        let (mut at, mut bytes) = (0u32, 0usize);
        for (i, (blend, shapes, text)) in self.layers.iter().enumerate() {
            if self.blur.is_some_and(|b| b.0 == i) {
                self.blur_split = at;
            }
            let start = at;
            if self.overlay_layer == Some(i) {
                self.overlay_vertex = start;
                fresh = true;
            }
            if self.model_layer == Some(i) {
                self.model_vertex = start;
                fresh = true;
            }
            let n = shapes.len() + text.len();
            at += Self::units(fmt, n);
            bytes += Self::stored_bytes(fmt, n);
            let end = at;
            match self.ranges.last_mut() {
                Some((b, r)) if b == blend && !fresh => r.end = end,
                _ if end > start => {
                    self.ranges.push((*blend, start..end));
                    fresh = false;
                }
                _ => {}
            }
        }
        self.frame_vertices = at;
        if bytes == 0 {
            return;
        }
        if self.vb.as_ref().is_none_or(|(_, cap)| *cap < bytes as u64) {
            let cap = (bytes as u64).next_power_of_two();
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("interface"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vb = Some((b, cap));
            self.sent.clear();
        }
        if fmt == Format::Indexed {
            let quads = at / 6;
            if self.ib.as_ref().is_none_or(|(_, n)| *n < quads) {
                // Static indices (0, 1, 2, 2, 3, 0) + 4k: same vertex order as the
                // triangles of the old path (provoking vertex included).
                let n = quads.next_power_of_two();
                let idx: Vec<u32> = (0..n).flat_map(|k| [0, 1, 2, 2, 3, 0].map(|i| 4 * k + i)).collect();
                let b = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("interface (index)"),
                    contents: bytemuck::cast_slice(&idx),
                    usage: wgpu::BufferUsages::INDEX,
                });
                self.ib = Some((b, n));
            }
        }
        let Some((vb, _)) = &self.vb else { return };
        if self.opts.keep {
            // Only the range changed since the previous frame goes out (nothing if the frame is identical).
            let mut cur = std::mem::take(&mut self.staging);
            cur.clear();
            Self::encode(&self.layers, fmt, &mut |b| cur.extend_from_slice(b));
            if let Some((lo, hi)) = changed_range(&self.sent, &cur) {
                up.write(queue, vb, lo as u64, &cur[lo..hi]);
            }
            self.staging = std::mem::replace(&mut self.sent, cur);
        } else if self.opts.write_with {
            let layers = &self.layers;
            up.write_with(queue, vb, 0, bytes as u64, |mut w| {
                let mut o = 0;
                Self::encode(layers, fmt, &mut |b| {
                    w.slice(o..o + b.len()).copy_from_slice(b);
                    o += b.len();
                });
            });
        } else {
            let mut all = std::mem::take(&mut self.staging);
            all.clear();
            Self::encode(&self.layers, fmt, &mut |b| all.extend_from_slice(b));
            queue.write_buffer(vb, 0, &all);
            self.staging = all;
        }
    }

    /// Vertices from before the blur (`after` false) or after (true); without blur, everything is "before".
    /// `between` (camera overlays) is interleaved at the place marked by
    /// [`Gui::overlays_here`] if it falls in the drawn part (otherwise at the end of the part
    /// that goes to the end, without a marked place).
    pub fn draw_blur_part(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        after: bool,
        between: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) {
        self.draw_blur_part_models(pass, after, between, |_| {});
    }

    /// Like [`Gui::draw_blur_part`], with the interface's 3D models interleaved at the place
    /// marked by [`Gui::models_here`].
    pub fn draw_blur_part_models(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        after: bool,
        between: impl FnOnce(&mut wgpu::RenderPass<'_>),
        models: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) {
        let s = self.blur_split;
        let part = if after { s..u32::MAX } else { 0..s };
        let at = |layer: Option<usize>, vertex: u32| if layer.is_some() { vertex } else { u32::MAX };
        let mine = |split: u32| split >= part.start && (split < part.end || part.end == u32::MAX);
        let o = at(self.overlay_layer, self.overlay_vertex);
        let m = at(self.model_layer, self.model_vertex);
        // Models: only at their place (never at the end of a part without a marked place).
        let with_models = self.model_layer.is_some() && m >= part.start && m < part.end;
        if !self.opts.uniform {
            let mut calls: Vec<Between<'_>> = Vec::new();
            if mine(o) {
                calls.push((o, Box::new(between)));
            }
            if with_models {
                calls.push((m, Box::new(models)));
            }
            calls.sort_by_key(|c| c.0);
            return self.draw_part(pass, part, calls);
        }
        // Without allocation: two places at most, in order (stable sort: overlays first).
        let (mut between, mut models) = (Some(between), Some(models));
        let mut call_o = |p: &mut wgpu::RenderPass<'_>| {
            if let Some(f) = between.take() {
                f(p);
            }
        };
        let mut call_m = |p: &mut wgpu::RenderPass<'_>| {
            if let Some(f) = models.take() {
                f(p);
            }
        };
        let mut calls: [Option<BetweenRef<'_>>; 2] = [
            mine(o).then_some((o, &mut call_o as &mut dyn FnMut(&mut wgpu::RenderPass<'_>))),
            with_models.then_some((m, &mut call_m as &mut dyn FnMut(&mut wgpu::RenderPass<'_>))),
        ];
        // Places present first, then sorted by vertex.
        if calls[0].is_none() || matches!(&calls, [Some(a), Some(b)] if b.0 < a.0) {
            calls.swap(0, 1);
        }
        self.draw_part_ref(pass, part, calls);
    }

    /// Frame bindings (restored after each interleaved draw).
    fn bind_frame(&self, pass: &mut wgpu::RenderPass<'_>, vb: &wgpu::Buffer) {
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_bind_group(1, &self.item_bind.0, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        if self.opts.format() == Format::Indexed
            && let Some((ib, _)) = &self.ib
        {
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
        }
    }

    /// Draws the elements `r` (vertices, indices or instances) with blend `blend`.
    fn draw_range(&self, pass: &mut wgpu::RenderPass<'_>, blend: Blend, r: std::ops::Range<u32>) {
        match self.opts.format() {
            Format::Tri => {
                pass.set_pipeline(&self.pipelines[blend as usize]);
                pass.draw(r, 0..1);
            }
            Format::Indexed => {
                pass.set_pipeline(&self.pipelines[blend as usize]);
                pass.draw_indexed(r, 0, 0..1);
            }
            Format::Instanced => {
                let p = self.inst_pipelines.as_ref().expect("instance pipelines created with the option");
                pass.set_pipeline(&p[blend as usize]);
                pass.draw(0..6, r);
            }
        }
    }

    fn draw_part(&self, pass: &mut wgpu::RenderPass<'_>, part: std::ops::Range<u32>, calls: Vec<Between<'_>>) {
        let mut calls = std::collections::VecDeque::from(calls);
        let Some((vb, _)) = self.vb.as_ref().filter(|_| self.frame_vertices > 0) else {
            for (_, b) in calls {
                b(pass);
            }
            return;
        };
        self.bind_frame(pass, vb);
        for (blend, r) in &self.ranges {
            let r = r.start.max(part.start)..r.end.min(part.end);
            while calls.front().is_some_and(|c| r.start >= c.0) {
                if let Some((_, b)) = calls.pop_front() {
                    b(pass);
                    self.bind_frame(pass, vb);
                }
            }
            if r.start < r.end {
                self.draw_range(pass, *blend, r);
            }
        }
        for (_, b) in calls {
            b(pass);
        }
    }

    /// Like `draw_part`, interleaved draws stored in order without allocation.
    fn draw_part_ref(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        part: std::ops::Range<u32>,
        mut calls: [Option<BetweenRef<'_>>; 2],
    ) {
        let Some((vb, _)) = self.vb.as_ref().filter(|_| self.frame_vertices > 0) else {
            for (_, b) in calls.iter_mut().flatten() {
                b(pass);
            }
            return;
        };
        self.bind_frame(pass, vb);
        let mut next = 0;
        for (blend, r) in &self.ranges {
            let r = r.start.max(part.start)..r.end.min(part.end);
            while let Some((at, b)) = calls.get_mut(next).and_then(Option::as_mut)
                && r.start >= *at
            {
                b(pass);
                self.bind_frame(pass, vb);
                next += 1;
            }
            if r.start < r.end {
                self.draw_range(pass, *blend, r);
            }
        }
        for (_, b) in calls[next..].iter_mut().flatten() {
            b(pass);
        }
    }
}

/// Glyph advances for measuring and splitting (`rc_text::Metrics`).
impl rc_text::Metrics for Gui {
    fn advance(&self, font: &str, c: char) -> (f32, f32) {
        match rc_text::object_font(font) {
            Some(rc_text::ObjectFont::Atlas { atlas, .. }) if !rc_assets::gui::KNOWN_ATLASES.contains(&atlas) => {
                return (self.missing.advance, self.missing.offset);
            }
            Some(_) => return (8.0, 1.0),
            None => {}
        }
        let f = self.font_index(font).map(|f| &self.fonts[f].1);
        let a = match (self.opts.font_table, self.filter_fishy.get()) {
            (false, true) => f.and_then(|f| f.glyph_non_fishy(c as u32)).map(|g| (g.advance, g.offset)),
            (false, false) => f.and_then(|f| f.advance(c as u32)),
            (true, true) => f.and_then(|f| f.glyph_non_fishy_ref(c as u32)).map(|g| (g.advance, g.offset)),
            (true, false) => f.and_then(|f| f.advance_ref(c as u32)),
        };
        a.unwrap_or((self.missing.advance, self.missing.offset))
    }
}

/// Simple text line (flattened component then § codes).
pub fn plain(s: &str) -> Vec<StyledChar> {
    rc_text::styled_chars(&[rc_text::Segment { style: Default::default(), text: s.to_owned() }])
}
