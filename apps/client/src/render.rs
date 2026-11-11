//! wgpu rendering of milestone 1: one pass, one wireframe pipeline. One instance per face (the
//! shader rebuilds the corners), one buffer and one bind group per column, columns
//! outside the field of view discarded on the CPU, GPU time measured by timestamp.
//!
//! This file holds the [`Renderer`] state, its constants and its accessors; the work is split by concern
//! into the submodules below.
//! - `frame`: the per-frame `render` (cameras, passes, submission).
//! - `terrain_draw`, `oit_draw`: encoding of the terrain batches, improved transparency.
//! - `translucent_sort`: draw order of the translucent quads.
//! - `column_upload`, `regions`: meshed columns on the GPU and the region buffers they share.
//! - `frustum`, `lists`: field-of-view tests and the draw lists built from them.
//! - `pipelines`, `surface`: GPU objects built at startup, presentation surface.
//! - `capture`: frame readback (F2 and test captures); `upload`: staging of the buffer writes.

use hashbrown::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::gpu_time::Role;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;
use winit::window::Window;

use rc_assets::BlockModels;

mod capture;
mod column_upload;
mod fog_cull;
mod frame;
mod frame_visibility;
mod frustum;
mod lists;
mod oit_draw;
mod pipelines;
mod regions;
mod surface;
mod terrain_draw;
mod translucent_sort;
pub(crate) mod upload;

#[cfg(test)]
mod test_meshes;

pub use column_upload::{Prepared, in_place_gaps, prepare, prepare_from_base, prepare_moving_faces, region_tag};
pub use frame_visibility::FrameVisibility;
pub use frustum::{BoxVerdict, ParticleFrustum};
pub(crate) use pipelines::camera_layout;

use column_upload::ColumnGpu;
use pipelines::{TerrainPipelines, baked_faces_buffer, block_texture_array, terrain_pipelines, uniform_layout};
use regions::{RegionGpu, identity_buffer};
use surface::{SurfaceSetup, configure_surface, depth_view};
use translucent_sort::{TransScratch, sort_all};
use upload::Uploader;

/// Camera slots in the frame's uniforms (`upload::Slots`).
const CAM_WORLD: usize = 0;
const CAM_HAND: usize = 1;
const CAM_TOTEM: usize = 2;
const CAM_GUI: usize = 3;
const _: () = assert!(size_of::<CameraUniform>() == upload::SLOT);

/// Reversed depth, like the game: near plane at 1, far at 0, buffer cleared to 0, "nearer
/// or equal" test = `GreaterEqual`. The precision of the 32-bit float then stays roughly
/// relative to the distance: no more flicker of almost coplanar faces in the distance.
pub const DEPTH_CLEAR: f32 = 0.0;
/// Default depth test (passes if at least as near as what is already written).
pub const DEPTH_NEARER: wgpu::CompareFunction = wgpu::CompareFunction::GreaterEqual;

/// Depth offset towards the camera (values of the game: constant, slope), in the sense of the
/// reversed depth (positive = nearer).
pub const fn depth_bias(constant: i32, slope: f32) -> wgpu::DepthBiasState {
    wgpu::DepthBiasState { constant, slope_scale: slope, clamp: 0.0 }
}

/// Perspective projection of the world (straight view, reversed [0, 1] depth): near and
/// far planes swapped, like the game.
pub fn perspective(fov_deg: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    glam::camera::rh::proj::directx::perspective(fov_deg.to_radians(), aspect, far, near)
}

/// Layer offset of the game in perspective: the view is scaled by (1 − bias/4096) around
/// the eye (bias 1 = towards the camera: armors, trims, lines, shadows; −1 = towards the back).
/// Same pixel on screen, depth slightly nearer.
pub fn view_layering(eye: Vec3, bias: f32) -> Mat4 {
    Mat4::from_translation(eye) * Mat4::from_scale(Vec3::splat(1.0 - bias / 4096.0)) * Mat4::from_translation(-eye)
}

/// Node of the occlusion walk: (section, arrival directions, step).
type GraphNode = ((i32, i32, i32), u8, u16);
/// Node for F3: walk node, face connectivity, geometry present.
pub type OccNode = ((i32, i32, i32), u8, u16, rc_mesh::visibility::Vis, bool);

/// What a visibility walk hands back, from its background task or made on the spot.
struct WalkResult {
    /// The walk's buffers, lent to the task.
    graph: rc_mesh::visibility::Graph,
    /// Sections reached, sorted by column then height.
    reached: Vec<(i32, i32, i32)>,
    /// Duration of the walk, in milliseconds.
    ms: f64,
    /// With `RUSTCRAFT_OPT_REACHGRID`, the reached sections as a bit grid.
    grid: Option<lists::ReachGrid>,
    /// With `RUSTCRAFT_OPT_MESH_REACHED_COLUMN_INDEX`, the run of each column in `reached`.
    columns: Option<frustum::ReachedColumns>,
    /// Distance from the eye at which the walk stopped (infinite: nowhere, `fog_cull`).
    sight_limit: f64,
    /// Rank of the walk among those launched: a result older than the one in use is not applied.
    launch: u64,
}

/// What a walk is redone for: camera cell of 8 blocks, connectivity version, occlusion rules,
/// bottom of the world, sections per column, sight limit (its bits).
type WalkKey = ([i64; 3], u64, bool, i32, i32, u64);

/// Maximum radius (in columns) of the visibility walk; the columns present are already
/// limited by the server's view distance.
const VIEW_LIMIT: i32 = 64;

/// Layer bit that designates the second texture array (larger textures).
const BIG_LAYER: u32 = 0x8000;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    origin: [i32; 4],
    /// Shading per direction (bottom, top, north, south, west, east).
    shade: [f32; 6],
    /// GPU appearance fade (`RUSTCRAFT_OPT_FADEGPU`): frame instant
    /// ([`crate::terrain_shader::fade_clock`]) and fade duration, in µs.
    fade_now: u32,
    fade_len: f32,
    /// Eye in the vertex frame, fog, sky colour, view without translation.
    eye: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
    fog_sky: [f32; 2],
    /// Texel size (1 / side) of the main and large block texture arrays
    /// (`RUSTCRAFT_OPT_ATLASTEXELUNIFORM`); the shaders other than the terrain ignore it.
    atlas_texel: [f32; 2],
    sky_color: [f32; 4],
    sky_view_proj: [[f32; 4]; 4],
}

impl CameraUniform {
    /// Camera of a layer without terrain or fog (totem, interface models): the fade clock
    /// is not read there, it stays at 0 so that the unchanged upload is skipped.
    fn overlay(self, view_proj: [[f32; 4]; 4], eye: [f32; 4]) -> Self {
        CameraUniform { view_proj, eye, fog_color: [0.0; 4], fog: [1e9, 2e9, 1e9, 2e9], fade_now: 0, ..self }
    }
}

/// Environment of the frame: fog, sky colour, dome and dark disc to draw.
#[derive(Debug, Clone, Copy)]
pub struct FrameEnv {
    pub fog: rc_env::Fog,
    pub sky: [f32; 4],
    pub dome: bool,
    pub disc: bool,
    /// Celestial bodies and clouds.
    pub bodies: crate::sky::SkyFrame,
    /// Far plane: max(64 × render distance, 16 × cloud range), like the game.
    pub far: f32,
    /// Water or lava in the camera (field of view, `hand-camera.md` § 4.3).
    pub fluid: bool,
}

/// Side of a region in columns (4 bits per axis in the faces).
const REGION: i32 = 16;
/// Default duration of the section appearance fade (setting `chunkSectionFadeInTime`).
const FADE_SECS: f64 = 0.75;
/// Batch key of the global translucent buffer (no real region has this key).
const TRANS_KEY: (i32, i32) = (i32::MIN, i32::MIN);
const FACE_BYTES: u64 = 24;
const ORDER_BYTES: u64 = 4;

/// Sections reached by the walk: generation, bottom of the world, number of sections, set.
type ReachedSet = (u64, i32, i32, Arc<lists::Reach>);

/// Key of the walk radius: version of the columns, camera column.
type RadiusKey = (u64, (i32, i32));

/// Command batch: layer, fluid?, region, first command, count.
type Batch = (usize, bool, (i32, i32), u32, u32);

/// Dependencies of a frame's commands: eye, yaw, pitch (and sort field in the high bits),
/// revision of the columns, walk, frame size, bottom of the world, number of sections.
type DrawKey = ([u64; 3], u32, u64, u64, u64, (u32, u32), i32, i32);

/// Indirect draw command (format expected by the GPU).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawArgs {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
}

/// Indirect indexed draw command (format expected by the GPU).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawIndexedArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// Faces at most per indexed command: 16-bit indices (4 × 16,384 vertices), 192 KiB of indices.
const INDEXED_FACES: u32 = 1 << 14;

/// Starting size of the identity buffer (`FACEIDX`), in faces: 256 KiB.
const FACE_IDS_START: u32 = 1 << 16;

/// Indices of the first `faces` faces: triangles 0-1-2 then 0-2-3 of each, vertices 4f to 4f + 3.
/// Texel size (1 / side) of a square block texture array of side `side` (0: placeholder array of
/// side 1), as the camera uniform carries it for the terrain fragments.
fn texel_size(side: u32) -> f32 {
    1.0 / side.max(1) as f32
}

pub(crate) fn face_indices(faces: u32) -> Vec<u16> {
    (0..faces)
        .flat_map(|f| {
            let b = (f * 4) as u16;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect()
}

/// Connectivity of the sections of each column, input of the visibility walk.
type VisSnapshot = Arc<HashMap<(i32, i32), Arc<[rc_mesh::visibility::Vis]>>>;

/// GPU formats of the explicit quads, chosen once at startup (same image; `docs/perf.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuFormats {
    /// `RUSTCRAFT_OPT_FLUID96`: region fluids (lava, layers 0 and 1) in 96 bytes, a
    /// single colour for the 4 corners. Only if every offset model fits in a compact face and
    /// these faces are in force (otherwise 4-colour quads remain in those layers).
    pub fluid96: bool,
    /// `RUSTCRAFT_OPT_TRANSCOMPACT`: translucents sorted in 108 bytes, absolute column read from
    /// a slot (table of 65,536 columns), texture on 15 bits.
    pub trans_compact: bool,
}

/// Words of a quad depending on the format: old 32, compact fluid 24, compact translucent 27.
const OLD_WORDS: usize = 32;
const FLUID96_WORDS: usize = 24;
const TRANS_WORDS: usize = 27;
/// Column slots of the compact translucents (16 bits of the information word).
const TRANS_SLOTS: usize = 1 << 16;

impl GpuFormats {
    fn decide(all_compact: bool, textures: usize) -> Self {
        let on = |n: &str| rc_data::env_flags::read_os(n).is_none_or(|v| v != "0");
        GpuFormats {
            fluid96: on("RUSTCRAFT_OPT_FLUID96") && all_compact && crate::remesh::formats().0,
            trans_compact: on("RUSTCRAFT_OPT_TRANSCOMPACT") && textures <= 0x8000,
        }
    }

    /// Shader entry points: sorted translucents, region fluids.
    fn entries(self) -> (&'static str, &'static str) {
        (
            if self.trans_compact { "vs_sorted_c" } else { "vs_sorted" },
            if self.fluid96 { "vs_fluid_c" } else { "vs_fluid" },
        )
    }

    pub fn fluid_words(self) -> usize {
        if self.fluid96 { FLUID96_WORDS } else { OLD_WORDS }
    }

    pub fn trans_words(self) -> usize {
        if self.trans_compact { TRANS_WORDS } else { OLD_WORDS }
    }

    fn fluid_bytes(self) -> u64 {
        self.fluid_words() as u64 * 4
    }

    fn trans_bytes(self) -> u64 {
        self.trans_words() as u64 * 4
    }
}

/// Formats fixed once per launch. A read before they are set from the mesh tables fixes the
/// environment's defaults for the whole session (a meshing task started too early would do it):
/// the setting then reports it.
struct FormatsOnce {
    formats: std::sync::OnceLock<GpuFormats>,
    read_before_set: std::sync::atomic::AtomicBool,
}

impl FormatsOnce {
    const fn new() -> Self {
        FormatsOnce {
            formats: std::sync::OnceLock::new(),
            read_before_set: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn read(&self) -> GpuFormats {
        *self.formats.get_or_init(|| {
            self.read_before_set.store(true, std::sync::atomic::Ordering::Relaxed);
            GpuFormats::decide(true, 0)
        })
    }

    /// Formats in force; `Err` when a read already fixed the defaults before this setting.
    fn set(&self, decide: impl FnOnce() -> GpuFormats) -> Result<GpuFormats, GpuFormats> {
        let f = *self.formats.get_or_init(decide);
        if self.read_before_set.load(std::sync::atomic::Ordering::Relaxed) { Err(f) } else { Ok(f) }
    }
}

static GPU_FORMATS: FormatsOnce = FormatsOnce::new();

/// Formats in force (set at startup from the mesh tables; without it, from the environment).
pub fn gpu_formats() -> GpuFormats {
    GPU_FORMATS.read()
}

/// Sets the formats from the mesh tables (once per launch, before the first meshing task).
pub fn init_gpu_formats(t: &rc_mesh::blocks::Tables) -> GpuFormats {
    let set = GPU_FORMATS.set(|| {
        let f = GpuFormats::decide(t.offsets_all_compact(), t.models.texture_names.len());
        eprintln!("[opt] GPU formats: {f:?}");
        f
    });
    set.unwrap_or_else(|f| {
        eprintln!("GPU formats read before the mesh tables were known: {f:?} kept for the session");
        f
    })
}

/// Everything the renderer is built from besides the GPU context.
pub struct RendererAssets<'a> {
    pub models: &'a BlockModels,
    pub textures: &'a [Vec<rc_assets::texture::Image>],
    pub sky: &'a crate::sky::SkyAssets,
    pub gui: crate::gui::GuiAssets,
    pub entities: Option<(Arc<rc_data::VersionData>, crate::entity_render::EntityTextures)>,
    pub tables: &'a rc_mesh::blocks::Tables,
}

/// Waits until the GPU memory of the resources just released is given back (resource reload on a
/// kept device): the new renderer does not allocate on top of the old one.
pub fn wait_for_released_gpu_memory(device: &wgpu::Device) {
    let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
}

pub struct Renderer {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    /// Adapter of the device, handed back with it on a resource reload.
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    pipelines: [wgpu::RenderPipeline; 3],
    fluid_pipelines: [wgpu::RenderPipeline; 3],
    /// Translucents drawn in sorted order (indices to the region's fluid quads).
    sorted_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    sky: crate::sky::Sky,
    /// 2D interface, filled by the app before `render`.
    pub gui: crate::gui::Gui,
    /// Entity rendering (absent without extracted models).
    pub entities: Option<crate::entity_render::EntityRenderer>,
    pub tags: crate::nametag::NameTags,
    /// Sign text (`block-entities.md` § 3).
    pub block_text: crate::block_text::BlockText,
    /// Text displays (`entities-objects.md` § 13.8).
    pub text_displays: crate::text_display::TextDisplays,
    /// Beacon beams and End portals (`block-entities.md` § 4.9 to 4.11).
    pub block_fx: crate::block_fx::BlockFx,
    pub outline: crate::outline::Outline,
    /// Glow outline of the glowing entities.
    pub entity_outline: crate::entity_outline::EntityOutline,
    /// Mining cracks (`interactions.md` § 2.2).
    pub cracks: crate::crack::Cracks,
    /// Debug screen traces (F3+B, F3+G, 3D crosshair); empty: nothing is drawn.
    pub debug_lines: crate::debug_render::DebugLines,
    /// Adapter, effective presentation mode, indirect draw: F3 lines `system_specs`, `fps`.
    pub adapter_info: wgpu::AdapterInfo,
    /// Vendor, API and driver in the game's words (F3 `system_specs`, F3+L system report).
    pub gpu_text: crate::gpu_backend::GpuText,
    pub present_mode: wgpu::PresentMode,
    warned_surface: bool,
    /// Presentation modes offered by the surface (choice of vertical sync).
    present_modes_caps: Vec<wgpu::PresentMode>,
    /// Camera binding and block textures to redo when the filtering changes (`video.rs`).
    terrain_res: crate::video::TerrainRes,
    /// RGSS filtering of the block textures (`textureFiltering` = 1).
    pub rgss: bool,
    pub particles: crate::particles::ParticleRenderer,
    /// Rain, snow and world border (`weather_render.rs`).
    pub weather: crate::weather_render::WeatherRenderer,
    /// Title screen panorama, drawn when `show_panorama` (out-of-world screens).
    pub panorama: Option<crate::panorama::Panorama>,
    pub show_panorama: bool,
    /// Menu background blur (requested by the interface, `Gui::blur`).
    blur: crate::blur::Blur,
    /// Leash ropes (`leash.rs`) and screen effects of the next frame (`post_fx.rs`), with the
    /// menu blur radius for the `blur` effect; the list is consumed every frame.
    pub leash: crate::leash::LeashRenderer,
    post_fx: crate::post_fx::PostFx,
    pub post_effects: Vec<crate::post_fx::Effect>,
    pub post_blur: i32,
    /// Writes of the render buffers (shared staging, `upload.rs`).
    up: Uploader,
    /// End-of-frame `device.poll` removed (`RUSTCRAFT_OPT_NOPOLL`, default 1): the read
    /// callbacks arrive at the next submit.
    no_poll: bool,
    /// Cameras of the world, hand, totem and interface (slots `CAM_*`).
    cameras: upload::Slots<4>,
    bind: wgpu::BindGroup,
    /// Projection of the frame (field of view, shake and sway, hand), set by the app.
    pub view: crate::camera::RenderView,
    /// Camera of the hand pass (same binding as `bind`, own projection).
    hand_bind: wgpu::BindGroup,
    /// Camera of the interface 3D models (orthographic projection, `special.rs`).
    gui_bind: wgpu::BindGroup,
    /// Totem animation: projection of the hand alone, pose in the view frame.
    totem_bind: wgpu::BindGroup,
    /// Block textures (main, large), for the overlays.
    block_views: (wgpu::TextureView, wgpu::TextureView),
    /// Screen effects and camera overlays (`overlays.rs`).
    pub overlays: Option<crate::overlays::Overlays>,
    /// Portal and nausea distortion, after the shake and sway.
    pub distort: glam::Mat4,
    column_layout: wgpu::BindGroupLayout,
    columns: HashMap<(i32, i32), ColumnGpu>,
    /// Columns meshed without any face (End void, empty superflat): only their
    /// connectivity, so that the walk crosses them like the game's empty sections.
    empty_vis: HashMap<(i32, i32), Arc<[rc_mesh::visibility::Vis]>>,
    /// Connectivity of all the columns for the walk, redone only when `vis_rev`
    /// changes (a moving camera reuses the same copy).
    vis_snapshot: Option<(u64, VisSnapshot)>,
    /// Layer displayed per texture (animations), CPU-side copy and GPU buffer.
    anim: wgpu::Buffer,
    frames: Vec<[u32; 2]>,
    /// Model layer → real layer (bit `BIG_LAYER`: second array; `u32::MAX`: absent).
    remap: Vec<u32>,
    /// Occlusion sorting of the sections (setting, `RUSTCRAFT_OCCLUSION=0` to turn it off).
    pub occlusion: bool,
    /// CPU duration of the visibility sort of the last frame.
    pub vis_ms: f64,
    /// Buffers of the walk, absent while a background task uses them.
    graph: Option<rc_mesh::visibility::Graph>,
    /// Fixed GPU sizes (baked faces, textures with mips), for the memory report.
    quads_bytes: u64,
    atlas_bytes: u64,
    /// Directional shading of the current dimension.
    pub shade: [f32; 6],
    regions: HashMap<(i32, i32), RegionGpu>,
    /// Appearance fade: births, columns fading in, ring (`terrain_shader::FadeRing`).
    fade_ring: crate::terrain_shader::FadeRing,
    /// Duration of the appearance fade (`chunkSectionFadeInTime`, 0 = no fade).
    pub fade_secs: f64,
    fade: wgpu::Buffer,
    lightmap: wgpu::Texture,
    /// Same lightmap as 256 RGBA8 words, read by the terrain vertices under
    /// `RUSTCRAFT_OPT_LIGHTMAPBUFFER` (absent otherwise).
    lightmap_colors: Option<wgpu::Buffer>,
    /// Texel size of the two block texture arrays, for the camera uniform.
    atlas_texel: [f32; 2],
    /// Commands of the last frame and what they depend on: as long as camera, walk and
    /// columns are unchanged, they are identical and reused as is.
    draw_key: Option<DrawKey>,
    /// Series, batches and commands of the frame, cache of the column order (`lists`).
    lists: lists::Lists,
    drawn_cache: (usize, u64),
    /// Instanced and indexed commands as written in their GPU buffer
    /// (`RUSTCRAFT_OPT_ARGSKIP`: only the changed range is rewritten).
    sent_args: Vec<DrawArgs>,
    sent_indexed_draw_args: Vec<DrawIndexedArgs>,
    /// Radius of the walk and its key (version of the columns, camera column).
    radius_cache: Option<(RadiusKey, i32)>,
    /// Grid of the sections reached by the last walk to arrive (`RUSTCRAFT_OPT_REACHGRID`).
    reached_grid: Option<lists::ReachGrid>,
    /// Number of the current walk result (`reached`).
    reached_gen: u64,
    /// Copy of the walk nodes for F3 (generation of the copied walk), made only
    /// when an occlusion visualisation is active.
    occ_nodes: (u64, Vec<GraphNode>),
    /// Occlusion visualisation active at the previous frame (set by the F3 traces).
    pub occ_debug: bool,
    /// Sections reached (generation, bottom of the world, number of sections, set) for the
    /// visibility of the entities.
    reached_set: ReachedSet,
    /// Translucent quads of all the columns and their draw order, in a single pair of
    /// buffers: the translucent pass, ordered from the farthest to the nearest, has only one
    /// binding and one batch (each quad carries its absolute column, origin (0, 0)).
    trans: RegionGpu,
    /// Formats of the explicit quads (`GpuFormats`).
    formats: GpuFormats,
    /// Table of the compact translucents' columns and free slots (`free_slots`, then
    /// `next_slot` never served).
    trans_slots: wgpu::Buffer,
    free_slots: Vec<u16>,
    next_slot: usize,
    /// Translucent sort: last camera, bottom of the world, camera block, rotation cursor.
    sort_eye: Option<[f64; 3]>,
    sort_min_y: i32,
    /// Re-sorts all the translucent columns, even those whose order does not change the image.
    sort_all: bool,
    /// Contiguous indirect commands kept separate (`RUSTCRAFT_SPLITDRAWS=1`, comparison).
    split_draws: bool,
    /// Entities tested against the field of view only, not the walk (`RUSTCRAFT_ALLENTITIES=1`).
    all_entities: bool,
    sort_block: Option<[i32; 3]>,
    sort_cursor: usize,
    /// Translucent columns (sorted by key), version of the columns at their computation, and number of
    /// columns not yet sorted for the camera's current block.
    trans_keys: Vec<(i32, i32)>,
    trans_keys_rev: Option<u64>,
    unsorted: usize,
    /// Buffers of the per-section re-sort (`RUSTCRAFT_OPT_TRANSPOV`): order of a run of sections, sorts.
    sort_scratch: TransScratch,
    /// Buffer of the frame's indirect draw commands, and whether the GPU accepts them.
    indirect: wgpu::Buffer,
    indirect_ok: bool,
    /// Variants of the terrain shader (`terrain_shader`).
    shader_opts: crate::terrain_shader::ShaderOpts,
    /// Indexed faces (`RUSTCRAFT_OPT_FACEIDX`): shared indices, identity buffer and its size,
    /// the frame's indexed commands, their buffer, instanced commands of the fluids alone (those
    /// sent to the GPU), and (first, count) of each batch in the list of its kind.
    face_index: wgpu::Buffer,
    face_ids: (wgpu::Buffer, u32),
    indexed_draw_args: Vec<DrawIndexedArgs>,
    fluid_draw_args: Vec<DrawArgs>,
    indirect_idx: wgpu::Buffer,
    indexed_batch_ranges: Vec<(u32, u32)>,
    graph_tx: crossbeam_channel::Sender<WalkResult>,
    graph_rx: crossbeam_channel::Receiver<WalkResult>,
    /// Last walk result and its key (exact position, version of the columns, settings).
    reached: Vec<(i32, i32, i32)>,
    /// Run of each column in `reached`, made with it (`frustum::ReachedColumns`).
    reached_columns: Option<frustum::ReachedColumns>,
    /// Meshing view of the last frame drawn (`frustum::Renderer::sections_in_view`).
    meshing_view: Option<frustum::MeshingView>,
    /// Bumped each time a frame's meshing view differs from the previous one.
    meshing_view_rev: u64,
    reached_key: Option<WalkKey>,
    /// Sight limit of the walk in use: a frame whose fog shows farther redoes the walk on the spot.
    reached_sight_limit: f64,
    /// Walks launched so far, and the rank of the one in use.
    walk_launches: u64,
    applied_walk_launch: u64,
    /// Reached sections the terrain draw lists read: without those the fog hides whole.
    drawn_sections: fog_cull::DrawnSections,
    columns_rev: u64,
    /// Version of the sections' connectivity (column appeared, disappeared, or one of whose sections
    /// connects differently): the only walk key on the column side (hotspot no. 7).
    vis_rev: u64,
    /// Inputs of the last walk launched (check `RUSTCRAFT_MESHCHECK`).
    graph_args: Option<fog_cull::WalkInputs>,
    /// Duration of the last walk actually redone.
    pub graph_ms: f64,
    /// Sum and count of the CPU durations per frame since the last report.
    cpu_ms_sum: f64,
    cpu_frames: u32,
    timer: Option<crate::gpu_time::GpuTimer>,
    pub adapter_name: String,
    pub wireframe: bool,
    capture: Option<std::path::PathBuf>,
    can_capture: bool,
    /// Faces and columns actually drawn in the last frame.
    pub drawn: (usize, u64),
    /// Improved transparency requested (option); effective if the GPU blends RGBA32F.
    pub improved_transparency: bool,
    oit_ok: bool,
    oit: Option<crate::oit::Oit>,
    /// Translucent terrain in improved transparency: block faces, sorted fluids.
    terrain_oit: Option<[crate::oit::Set; 2]>,
}

impl Renderer {
    /// Renderer on `window` with the device of `gpu`. `between_phases` runs after each long phase
    /// (surface, block textures, each group of pipelines): the app applies there the session events
    /// that need no renderer and starts their meshing (`events_before_renderer.rs`).
    pub fn new(
        window: Arc<Window>,
        gpu: crate::gpu_backend::GpuContext,
        assets: RendererAssets<'_>,
        between_phases: &mut dyn FnMut(),
    ) -> Self {
        let RendererAssets {
            models,
            textures,
            sky: sky_assets,
            gui: gui_assets,
            entities: entity_assets,
            tables,
        } = assets;
        // Set at startup from these tables, before any meshing task (`startup.rs`).
        let formats = gpu_formats();
        let crate::gpu_backend::GpuContext { surface, adapter, device, queue, text: gpu_text } = gpu;
        let info = adapter.get_info();
        // Features retained when the device was opened (`gpu_backend::open_device`).
        let features = device.features();
        let wireframe = features.contains(wgpu::Features::POLYGON_MODE_LINE);
        // Wireframe on demand (RUSTCRAFT_WIRE=1), otherwise solid rendering.
        let wire_mode = wireframe && rc_data::env_str!("RUSTCRAFT_WIRE") == Some("1");
        let timestamps = features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let indirect_ok = features.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE);
        let oit_ok = features.contains(wgpu::Features::FLOAT32_BLENDABLE);
        eprintln!(
            "GPU features: indirect draw {}, timestamp {}, wireframe {}",
            if indirect_ok { "yes" } else { "no (one draw per series)" },
            if timestamps { crate::gpu_time::GpuTimer::mode_for(features).describe() } else { "no (GPU time absent)" },
            if wireframe { "yes" } else { "no" },
        );

        let SurfaceSetup { config, can_capture, present_mode, present_modes_caps } =
            configure_surface(&surface, &adapter, &device, &window);
        crate::boot::mark("renderer: surface configured");
        between_phases();

        let shader_opts = crate::terrain_shader::opts().within_limits(&device.limits());
        let cameras = upload::Slots::new(
            &device,
            ["camera", "hand camera", "totem animation camera", "interface model camera"],
            upload::merged_on(),
            upload::skip_on(),
        );
        let up = Uploader::new(&device, upload::ring_on());
        // Baked faces of all the models (read by the shader).
        let quads = baked_faces_buffer(&device, models);
        // Block textures: two 2D arrays with mipmaps, one at the main size, the other
        // for the rare larger textures (bit 15 of the layer). Beyond the GPU limit,
        // the surplus images are dropped (the texture stays fixed).
        // Offsets of the compact face models (`offset_table`).
        let offsets = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model offsets"),
            contents: bytemuck::cast_slice(&tables.offset_table()),
            usage: wgpu::BufferUsages::STORAGE,
        });
        // Absolute columns of the compact translucents' slots.
        let trans_slots = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("translucent columns"),
            size: if formats.trans_compact { TRANS_SLOTS as u64 * 8 } else { 8 },
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quads_bytes = quads.size() + offsets.size() + trans_slots.size();
        let max_layers = device.limits().max_texture_array_layers;
        let (main_size, big_size) = (models.texture_size, models.big_texture_size);
        let mut remap: Vec<u32> = Vec::with_capacity(textures.len());
        let (mut main_layers, mut big_layers): (Vec<&Vec<rc_assets::texture::Image>>, Vec<_>) =
            (Vec::new(), Vec::new());
        for t in textures {
            let big = big_size > 0 && t[0].width == big_size;
            let list = if big { &mut big_layers } else { &mut main_layers };
            if (list.len() as u32) < max_layers {
                remap.push(list.len() as u32 | if big { BIG_LAYER } else { 0 });
                list.push(t);
            } else {
                remap.push(u32::MAX);
            }
        }
        crate::boot::mark("renderer: block texture arrays started");
        let (atlas_view, main_bytes) = block_texture_array(&device, &queue, "block textures", main_size, &main_layers);
        let (big_view, big_bytes) =
            block_texture_array(&device, &queue, "large block textures", big_size.max(1), &big_layers);
        let atlas_texel = [texel_size(main_size), texel_size(big_size)];
        crate::boot::mark("block textures written (write_texture)");
        between_phases();
        let atlas_bytes = main_bytes + big_bytes;
        let sampler = crate::video::atlas_sampler(&device, 4, 1);
        let camera_layout = camera_layout(&device, shader_opts.lightmap_buffer);
        // Animation: per texture, (displayed layer | next layer << 16, fade in thousandths).
        let layer_of = |i: u32| remap.get(i as usize).copied().filter(|&l| l != u32::MAX).unwrap_or(0);
        let frames: Vec<[u32; 2]> =
            (0..models.texture_names.len().max(1) as u32).map(|i| [layer_of(i) | layer_of(i) << 16, 0]).collect();
        let anim = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture animation"),
            contents: bytemuck::cast_slice(&frames),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let column_layout = uniform_layout(&device);
        // Fade ring: visibilities (1 = visible) or birth instants (finished = sentinel).
        let fade_ring = crate::terrain_shader::FadeRing::new(shader_opts.fadegpu, Instant::now());
        let fade = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("appearance fade"),
            contents: bytemuck::cast_slice(fade_ring.ring()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        // Indexed faces: shared indices [0, 1, 2, 0, 2, 3] + 4 × face, and identity buffer (the
        // first face of a command arrives through its instance attribute).
        let face_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("face indices"),
            contents: bytemuck::cast_slice(&face_indices(INDEXED_FACES)),
            usage: wgpu::BufferUsages::INDEX,
        });
        let face_ids = identity_buffer(&device, FACE_IDS_START);
        // Lightmap 16 × 16 (column = block, row = sky), white until the first computation.
        let lightmap = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lightmap"),
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let lightmap_view = lightmap.create_view(&Default::default());
        // Zeroed like the texture until the first computation.
        let lightmap_colors = shader_opts.lightmap_buffer.then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lightmap colours"),
                size: 256 * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmap"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Camera binding resources, kept to redo it when the sampler changes.
        let terrain_res = crate::video::TerrainRes {
            layout: camera_layout.clone(),
            quads: quads.clone(),
            atlas: atlas_view.clone(),
            big: big_view.clone(),
            anim: anim.clone(),
            fade: fade.clone(),
            lightmap: lightmap_view.clone(),
            lightmap_sampler: lightmap_sampler.clone(),
            lightmap_colors: lightmap_colors.clone(),
            offsets,
            key: (4, 1),
        };
        let make_bind = |i: usize| terrain_res.bind(&device, cameras.binding(i), &sampler);
        let bind = make_bind(CAM_WORLD);
        let hand_bind = make_bind(CAM_HAND);
        let gui_bind = make_bind(CAM_GUI);
        let totem_bind = make_bind(CAM_TOTEM);
        let TerrainPipelines { blocks: pipelines, fluids: fluid_pipelines, sorted: sorted_pipeline, sky: sky_pipeline } =
            terrain_pipelines(&device, &camera_layout, &column_layout, config.format, wire_mode, shader_opts, formats);
        crate::boot::mark("renderer: terrain and sky pipelines");
        between_phases();
        let timer = timestamps.then(|| {
            let mode = crate::gpu_time::GpuTimer::mode_for(features);
            crate::gpu_time::GpuTimer::new(&device, &queue, mode, crate::gpu_time::settings().passes)
        });
        let (graph_tx, graph_rx) = crossbeam_channel::unbounded();
        let trans = RegionGpu::new(&device, &column_layout, (0, 0), formats.trans_bytes(), &trans_slots);
        let sky = crate::sky::Sky::new(&device, &queue, config.format, sky_assets);
        crate::boot::mark("renderer: sky");
        let gui = crate::gui::Gui::new(&device, &queue, config.format, gui_assets);
        crate::boot::mark("renderer: interface");
        between_phases();
        let blur = crate::blur::Blur::new(&device, config.format);
        crate::boot::mark("renderer: blur");
        let leash = crate::leash::LeashRenderer::new(&device, &camera_layout, config.format);
        let post_fx = crate::post_fx::PostFx::new(&device, config.format);
        crate::boot::mark("renderer: leashes and screen effects");
        let tags = crate::nametag::NameTags::new(&device, &camera_layout, config.format);
        let block_text = crate::block_text::BlockText::new(&device, &camera_layout, config.format);
        let text_displays = crate::text_display::TextDisplays::new(&device, &camera_layout, config.format);
        let block_fx = crate::block_fx::BlockFx::new(&device, &camera_layout, config.format);
        let outline = crate::outline::Outline::new(&device, &camera_layout, config.format);
        let entity_outline = crate::entity_outline::EntityOutline::new(&device, config.format);
        let cracks = crate::crack::Cracks::new(&device, &camera_layout, config.format);
        let debug_lines = crate::debug_render::DebugLines::new(&device, &camera_layout, config.format);
        crate::boot::mark("renderer: labels, texts, outlines, cracks and debug lines");
        between_phases();
        // Block fragments: layers of the main array only.
        let particle_layers =
            remap.iter().map(|&l| if l == u32::MAX || l & BIG_LAYER != 0 { u32::MAX } else { l }).collect();
        let particles =
            crate::particles::ParticleRenderer::new(&device, &camera_layout, config.format, particle_layers);
        crate::boot::mark("renderer: particles");
        let entities = entity_assets.map(|(data, tex)| {
            crate::entity_render::EntityRenderer::new(&device, &camera_layout, config.format, data, tex)
        });
        crate::boot::mark("renderer: entities");
        between_phases();
        let weather = crate::weather_render::WeatherRenderer::new(&device, &camera_layout, config.format);
        crate::boot::mark("renderer: weather");
        Renderer {
            weather,
            anim,
            frames,
            remap,
            occlusion: rc_data::env_str!("RUSTCRAFT_OCCLUSION") != Some("0"),
            vis_ms: 0.0,
            graph: Some(Default::default()),
            quads_bytes,
            atlas_bytes,
            shade: rc_mesh::wire::SHADE_DEFAULT,
            regions: HashMap::new(),
            fade_ring,
            fade_secs: FADE_SECS,
            fade,
            lightmap,
            lightmap_colors,
            atlas_texel,
            draw_key: None,
            lists: lists::Lists::default(),
            drawn_cache: (0, 0),
            sent_args: Vec::new(),
            sent_indexed_draw_args: Vec::new(),
            radius_cache: None,
            reached_grid: None,
            reached_gen: 0,
            occ_nodes: (u64::MAX, Vec::new()),
            occ_debug: false,
            reached_set: (0, 0, 0, Arc::default()),
            trans,
            formats,
            trans_slots,
            free_slots: Vec::new(),
            next_slot: 0,
            sort_eye: None,
            sort_min_y: 0,
            sort_all: sort_all(),
            split_draws: rc_data::env_flag!("RUSTCRAFT_SPLITDRAWS"),
            all_entities: rc_data::env_flag!("RUSTCRAFT_ALLENTITIES"),
            sort_block: None,
            sort_cursor: 0,
            trans_keys: Vec::new(),
            trans_keys_rev: None,
            unsorted: 0,
            sort_scratch: TransScratch::default(),
            indirect: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indirect commands"),
                size: 1 << 16,
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            indirect_ok,
            shader_opts,
            face_index,
            face_ids: (face_ids, FACE_IDS_START),
            indexed_draw_args: Vec::new(),
            fluid_draw_args: Vec::new(),
            indirect_idx: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indirect indexed commands"),
                size: 1 << 16,
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            indexed_batch_ranges: Vec::new(),
            graph_tx,
            graph_rx,
            reached: Vec::new(),
            reached_columns: None,
            meshing_view: None,
            meshing_view_rev: 0,
            reached_key: None,
            reached_sight_limit: f64::INFINITY,
            walk_launches: 0,
            applied_walk_launch: 0,
            drawn_sections: Default::default(),
            columns_rev: 0,
            vis_rev: 0,
            graph_args: None,
            graph_ms: 0.0,
            cpu_ms_sum: 0.0,
            cpu_frames: 0,
            depth: depth_view(&device, config.width, config.height),
            window,
            surface,
            adapter,
            device,
            queue,
            config,
            pipelines,
            fluid_pipelines,
            sorted_pipeline,
            sky_pipeline,
            sky,
            gui,
            entities,
            tags,
            block_text,
            text_displays,
            block_fx,
            outline,
            entity_outline,
            cracks,
            debug_lines,
            adapter_info: info.clone(),
            gpu_text,
            present_mode,
            warned_surface: false,
            present_modes_caps,
            terrain_res,
            rgss: false,
            panorama: None,
            show_panorama: false,
            blur,
            leash,
            post_fx,
            post_effects: Vec::new(),
            post_blur: 5,
            particles,
            up,
            no_poll: rc_data::env_str!("RUSTCRAFT_OPT_NOPOLL") != Some("0"),
            cameras,
            bind,
            view: crate::camera::RenderView::default(),
            hand_bind,
            gui_bind,
            totem_bind,
            block_views: (atlas_view.clone(), big_view.clone()),
            overlays: None,
            distort: glam::Mat4::IDENTITY,
            column_layout,
            columns: HashMap::new(),
            empty_vis: HashMap::new(),
            vis_snapshot: None,
            timer,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            wireframe,
            capture: None,
            can_capture,
            drawn: (0, 0),
            improved_transparency: false,
            oit_ok,
            oit: None,
            terrain_oit: None,
        }
    }

    /// Hands back surface, adapter, device and queue, and releases everything else this renderer
    /// built (resource reload, `RUSTCRAFT_OPT_KEEP_GPU_ON_RELOAD`): the next renderer is built
    /// whole on the same device and the same surface, without reopening the driver.
    pub fn into_gpu_context(self) -> crate::gpu_backend::GpuContext {
        let Renderer { surface, adapter, device, queue, gpu_text, .. } = self;
        crate::gpu_backend::GpuContext { surface, adapter, device, queue, text: gpu_text }
    }

    /// Sampler of the block textures: mip levels (`mipmapLevels`) and anisotropy
    /// (1 = none); the three camera bindings are redone if the key changes.
    pub fn set_atlas_sampler(&mut self, mips: u32, aniso: u16) {
        let aniso = aniso.clamp(1, 16);
        if self.terrain_res.key == (mips, aniso) {
            return;
        }
        self.terrain_res.key = (mips, aniso);
        let s = crate::video::atlas_sampler(&self.device, mips, aniso);
        let bind = |i| self.terrain_res.bind(&self.device, self.cameras.binding(i), &s);
        (self.bind, self.hand_bind, self.gui_bind, self.totem_bind) =
            (bind(CAM_WORLD), bind(CAM_HAND), bind(CAM_GUI), bind(CAM_TOTEM));
        // Commands reused from the previous frame: invalidated.
        self.columns_rev += 1;
    }

    /// Tick animation frames: (texture, displayed layer, next layer, fade ‰).
    pub fn animate(&mut self, steps: impl Iterator<Item = (u32, u32, u32, u32)>) {
        let mut changed = false;
        for (tex, cur, next, milli) in steps {
            let map = |l: u32| self.remap.get(l as usize).copied().filter(|&v| v != u32::MAX);
            let ok = |l: u32| map(l).or_else(|| map(tex)).unwrap_or(0);
            let v = [ok(cur) | ok(next) << 16, milli];
            if let Some(e) = self.frames.get_mut(tex as usize)
                && *e != v
            {
                *e = v;
                changed = true;
            }
        }
        if changed {
            self.up.write(&self.queue, &self.anim, 0, bytemuck::cast_slice(&self.frames));
        }
    }

    /// The format of the region fluids depends on the compact offset faces (`GpuFormats`).
    pub fn fluid96(&self) -> bool {
        self.formats.fluid96
    }

    /// Enables the title screen panorama (test: drawn over the world).
    pub fn enable_panorama(&mut self, assets: &crate::panorama::PanoramaAssets) {
        self.panorama = Some(crate::panorama::Panorama::new(&self.device, &self.queue, self.config.format, assets));
    }

    /// Screen effects and camera overlays (`overlays.rs`).
    pub fn enable_overlays(
        &mut self,
        assets: Arc<rc_assets::Assets>,
        models: Arc<BlockModels>,
        data: Arc<rc_data::VersionData>,
    ) {
        let views = (&self.block_views.0, &self.block_views.1);
        let o =
            crate::overlays::Overlays::new(&self.device, &self.queue, self.config.format, views, assets, models, data);
        self.overlays = Some(o);
    }

    /// Vertices of the frame's overlays (current layers of the animated block textures).
    pub fn prepare_overlays(&mut self, f: &crate::overlays::Frame<'_>) {
        let (frames, size, scale) = (&self.frames, (self.config.width, self.config.height), self.gui.scale);
        if let Some(o) = &mut self.overlays {
            o.prepare(&self.device, &self.queue, &mut self.up, f, size, scale, |t| {
                frames.get(t as usize).map(|v| v[0] & 0xffff)
            });
        }
    }

    /// Textures of the beams and End portals.
    pub fn load_block_fx(&mut self, assets: &rc_assets::Assets) {
        let load = |p: &str| {
            assets
                .read(&format!("assets/minecraft/textures/{p}.png"))
                .and_then(|b| rc_assets::texture::Image::decode_png(&b))
                .map(|i| i.first_frame())
                .unwrap_or_else(|| rc_assets::texture::Image { width: 1, height: 1, rgba: vec![255; 4] })
        };
        let imgs = [
            load("entity/beacon/beacon_beam"),
            load("entity/end_portal/end_gateway_beam"),
            load("entity/end_portal/end_sky"),
            load("entity/end_portal/end_portal"),
        ];
        self.block_fx.set_textures(&self.device, &self.queue, [&imgs[0], &imgs[1], &imgs[2], &imgs[3]]);
    }

    /// Check (`RUSTCRAFT_MESHCHECK`): fingerprint of each column uploaded, and the last
    /// walk result compared with a walk redone on the current columns (`None` if no
    /// walk).
    pub fn mesh_check(&self) -> (HashMap<(i32, i32), u64>, Option<bool>) {
        let digests = self.columns.iter().map(|(&k, c)| (k, c.digest)).collect();
        let reached = self.graph_args.map(|inputs| {
            let vis = |x, z| {
                let empty = || self.empty_vis.get(&(x, z)).map(|v| &v[..]);
                self.columns.get(&(x, z)).map(|c| &c.vis[..]).or_else(empty)
            };
            let fog_cull::WalkInputs { eye, min_y, sections, radius, occlusion, sight_limit } = inputs;
            // Walked whole, then cut at the sight limit: also checks the walk that stops there itself.
            let mut graph = rc_mesh::visibility::Graph::default();
            let whole = graph.run(vis, eye, min_y, sections, radius, occlusion);
            let beyond_sight = |pos| rc_mesh::visibility::beyond_sight(eye, min_y, pos, sight_limit);
            let mut r: Vec<_> = whole.iter().copied().filter(|&pos| !beyond_sight(pos)).collect();
            r.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
            r == self.reached
        });
        (digests, reached)
    }

    /// F3 (paths and section visibility): per section reached by the last walk,
    /// (section, arrival directions, step, face connectivity, geometry present). The
    /// nodes are copied when a new walk has come back (the old copy serves while
    /// a walk runs in the background).
    pub fn occlusion_debug(&mut self) -> Vec<OccNode> {
        self.occ_debug = true;
        self.keep_occ_nodes();
        self.occ_nodes
            .1
            .iter()
            .map(|&(p, sources, step)| {
                let col = self.columns.get(&(p.0, p.2));
                let vis = col.and_then(|c| c.vis.get(p.1 as usize).copied()).unwrap_or(rc_mesh::visibility::VIS_NONE);
                let s = p.1 as usize;
                let any = |starts: &[Vec<u32>; 3]| starts.iter().any(|v| v.len() > s + 1 && v[s + 1] > v[s]);
                let geometry = col.is_some_and(|c| any(&c.starts) || any(&c.fluid_starts));
                (p, sources, step, vis, geometry)
            })
            .collect()
    }

    /// Copy of the nodes of the walk that came back, if an occlusion visualisation asks for it.
    fn keep_occ_nodes(&mut self) {
        if let Some(g) = &self.graph
            && self.occ_debug
            && self.occ_nodes.0 != self.reached_gen
        {
            self.occ_nodes.1.clear();
            self.occ_nodes.1.extend(g.nodes());
            self.occ_nodes.0 = self.reached_gen;
        }
    }

    /// Number of columns in GPU memory and the corresponding faces.
    pub fn stats(&self) -> (usize, u64) {
        (self.columns.len(), self.columns.values().map(|c| u64::from(c.face_alloc.1)).sum())
    }

    /// Columns having translucent quads, and among them those never re-sorted.
    pub fn trans_stats(&self) -> (usize, usize) {
        let with = self.columns.values().filter(|c| c.trans_alloc.1 > 0);
        with.fold((0, 0), |(n, f), c| (n + 1, f + usize::from(c.trans_centers.is_empty())))
    }

    /// CPU copies of the translucent quads (to re-sort them), in bytes.
    #[cfg_attr(not(feature = "memstats"), allow(dead_code))]
    pub fn trans_bytes(&self) -> usize {
        self.columns.values().map(|c| c.trans_centers.capacity() * 12 + c.order_starts.capacity() * 4).sum()
    }

    /// GPU memory allocated by us, in bytes: (region buffers allocated, of which used,
    /// textures, other buffers and depth).
    /// Report of the GPU allocator (backends that provide it).
    pub fn allocator_report(&self) -> Option<wgpu::AllocatorReport> {
        self.device.generate_allocator_report()
    }

    pub fn vram(&self) -> (u64, u64, u64, u64) {
        let alloc: u64 = self.regions.values().map(|r| r.faces.size() + r.fluids.size()).sum::<u64>()
            + self.trans.fluids.size()
            + self.trans.order.size();
        let used: u64 = self
            .columns
            .values()
            .map(|c| {
                u64::from(c.face_alloc.1) * FACE_BYTES
                    + u64::from(c.fluid_alloc.1) * self.formats.fluid_bytes()
                    + u64::from(c.trans_alloc.1) * self.formats.trans_bytes()
                    + u64::from(c.order_alloc.1) * ORDER_BYTES
            })
            .sum();
        let depth = u64::from(self.config.width) * u64::from(self.config.height) * 4;
        // Indexed faces (`FACEIDX`): shared indices, indexed commands, identity buffer.
        let faceidx = self.face_index.size() + self.indirect_idx.size() + self.face_ids.0.size();
        (alloc, used, self.atlas_bytes, self.quads_bytes + self.indirect.size() + self.anim.size() + faceidx + depth)
    }

    /// Average GPU time of a frame since the last call (ms), if measured.
    /// Average CPU duration per frame since the last call (ms).
    /// Replaces the lightmap (16 × 16 RGBA8, row = sky level).
    pub fn set_lightmap(&self, rgba: &[u8]) {
        self.queue.write_texture(
            self.lightmap.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(64), rows_per_image: Some(16) },
            wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
        );
        if let Some(colors) = &self.lightmap_colors {
            self.queue.write_buffer(colors, 0, rgba);
        }
    }

    /// Command queue (for sending the glyphs from the app).
    pub fn queue_handle(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    /// Window size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Width / height ratio of the frame.
    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    pub fn cpu_ms(&mut self) -> f64 {
        let v = if self.cpu_frames == 0 { 0.0 } else { self.cpu_ms_sum / f64::from(self.cpu_frames) };
        (self.cpu_ms_sum, self.cpu_frames) = (0.0, 0);
        v
    }

    /// Terrain in grouped indirect draws (F3 line `Terrain Rendering`).
    pub fn uses_indirect(&self) -> bool {
        self.indirect_ok
    }

    pub fn gpu_ms(&mut self) -> Option<f64> {
        let pixels = u64::from(self.config.width) * u64::from(self.config.height);
        self.timer.as_mut().and_then(|t| t.average_ms(pixels))
    }

    /// Timestamps of a named pass (GPU time), `None` without measurement in this frame.
    fn stamp(&self, name: &'static str, role: Role) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.timer.as_ref().and_then(|t| t.pass(name, role))
    }
}

#[cfg(test)]
#[path = "render_gpu_test.rs"]
mod gpu_test;

#[cfg(test)]
mod kept_device_test;

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec4;

    /// A read of the GPU formats before they are set from the mesh tables (a meshing task started
    /// too early) is reported by the setting; set first, the formats read are those of the tables.
    /// Sabotage checked: ignoring the early read in `set` fails the first assertion.
    #[test]
    fn formats_read_before_being_set_are_reported() {
        let tables = GpuFormats { fluid96: false, trans_compact: false };
        let early = FormatsOnce::new();
        let defaults = early.read();
        assert_eq!(early.set(|| tables), Err(defaults), "early read not reported");
        let in_order = FormatsOnce::new();
        assert_eq!(in_order.set(|| tables), Ok(tables));
        assert_eq!(in_order.read(), tables);
        assert_eq!(in_order.set(|| unreachable!("set twice")), Ok(tables), "second setting keeps the first");
    }

    /// Totem and interface do not read the fade clock: their cameras do not change
    /// when only the world clock advances (unchanged upload skipped).
    #[test]
    fn overlay_camera_ignores_the_fade_clock() {
        let base = |fade_now| CameraUniform { fade_now, fade_len: 5e5, shade: [0.5; 6], ..bytemuck::Zeroable::zeroed() };
        let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
        let (a, b) = (base(1_000).overlay(vp, [0.0; 4]), base(17_667).overlay(vp, [0.0; 4]));
        assert_eq!(bytemuck::bytes_of(&a), bytemuck::bytes_of(&b));
        assert_eq!((a.fade_len, a.shade, a.fog), (5e5, [0.5; 6], [1e9, 2e9, 1e9, 2e9]));
    }

    /// Depth (after division) of a point `d` blocks in front of the camera.
    fn depth_at(proj: Mat4, d: f32) -> f32 {
        let c = proj * Vec4::new(0.0, 0.0, -d, 1.0);
        c.z / c.w
    }

    #[test]
    fn depth_is_reversed_like_the_game() {
        let p = perspective(70.0, 16.0 / 9.0, 0.05, 512.0);
        assert!((depth_at(p, 0.05) - 1.0).abs() < 1e-6);
        assert!(depth_at(p, 512.0).abs() < 1e-6);
        assert!(depth_at(p, 10.0) > depth_at(p, 11.0), "nearer = greater");
        assert_eq!(DEPTH_CLEAR, 0.0);
        assert_eq!(DEPTH_NEARER, wgpu::CompareFunction::GreaterEqual);
    }

    /// Carpet (1/16 of a block) above the ground at 300 blocks: in classic depth the two faces
    /// fall on the same float or nearly (flicker), in reversed depth they
    /// stay thousands of steps apart.
    #[test]
    fn far_coplanar_faces_stay_apart() {
        let ulps = |a: f32, b: f32| (a.to_bits() as i64 - b.to_bits() as i64).abs();
        let (d0, d1) = (300.0, 300.0 + 1.0 / 16.0);
        let classic = glam::camera::rh::proj::directx::perspective(70f32.to_radians(), 16.0 / 9.0, 0.05, 512.0);
        assert!(ulps(depth_at(classic, d0), depth_at(classic, d1)) <= 2);
        let reversed = perspective(70.0, 16.0 / 9.0, 0.05, 512.0);
        assert!(ulps(depth_at(reversed, d0), depth_at(reversed, d1)) > 1000);
    }
}
