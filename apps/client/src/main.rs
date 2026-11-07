//! Rustcraft, milestone 1: offline connection, receiving the world, wireframe display,
//! free camera (ZQSD/WASD, space/shift, mouse after a click, Esc to release).
//!
//! `rustcraft [--username name]`: title screen; `rustcraft host [port] [name]`: direct
//! connection (port 25565, name Rustcraft by default).

mod alloc_trace;
mod arena;
mod audio_slot;
mod block_anim;
mod bone_meal_use;
mod boot;
mod block_entities;
mod block_fx;
mod block_text;
mod cache_key;
mod capseq;
mod disconnect_report;
mod entity_loops;
mod entity_motion;
mod blur;
mod camera;
mod chat;
mod commands;
mod crack;
mod crash_report;
mod creative;
mod creative_app;
mod creative_screen;
mod advancements;
mod advancements_screen;
mod stats;
mod stats_screen;
mod progress_app;
mod death;
mod debug_charts;
mod debug_keys;
mod debug_options;
mod debug_render;
mod debug_screen;
mod debug_switcher;
mod debug_sys;
mod dialog;
mod misc;
mod entities;
mod entity_atlas;
mod entity_outline;
mod entity_render;
mod environ;
mod events_before_renderer;
mod exact;
mod explode;
mod flow;
mod frame_stages;
mod front;
mod glyph_pool;
mod gui;
mod hand;
mod hotbars;
mod hud;
mod hud_extra;
mod hud_more;
mod ime;
mod interact;
mod inventory;
mod item_atlas;
mod item_use;
mod quickplay;
mod keynames;
mod keys;
mod launch;
mod leash;
mod level_events;
mod light_engine;
mod remesh;
mod links;
mod maps;
mod menu;
mod menus;
mod nametag;
mod oit;
mod options;
mod outline;
mod overlays;
mod pack_screen;
mod pack_stack;
mod panorama;
mod particles;
mod pause;
mod perf_log;
mod pick;
mod piston_replay;
mod profiler;
mod profiling_zip;
mod projectile_sim;
mod player_env;
mod players;
mod post_fx;
mod present_watch;
mod recipe_app;
mod recipe_book;
mod reload;
mod render;
mod resources;
mod riding;
mod scoreboard;
mod screens;
mod screenshot;
mod server_list;
mod server_packs;
mod servers;
mod skins;
mod sky;
mod sky_mesh;
mod item_regs;
mod spear_anim;
mod stack;
mod subtitles;
mod symlink_notice;
mod tablist;
mod tech_blocks;
mod terrain_shader;
mod window_title;
mod text_display;
mod syslocale;
mod ticking;
mod toasts;
mod tooltips;
mod tooltips_items;
mod tooltips_more;
mod weather;
mod weather_render;
mod video;
mod gpu_backend;
mod gpu_time;
#[allow(dead_code)]
mod widgets;
mod world_sounds;
// Special screens (`special-screens.md`).
mod book;
mod menu_special;
mod op_screens;
mod screens_mount;
mod screens_trade;
mod screens_work;
mod sign_edit;
mod special;
mod create_world;
mod edit_world;
mod world_list;
// End of the game, sleeping, spectator, spyglass (M5).
mod bed;
mod spectator;
mod sprite_anim;
mod win_screen;
// World options, options screens and warnings (M3).
mod credits_roll;
mod notices;
mod world_options;
// Items, effects and notifications (M6).
mod bundle;
mod inv_effects;
mod item_special;
mod tutorial;
mod world_fx;
mod entity_events;
mod totem_anim;
mod entity_fx;

// The application, one file per concern: `impl App` blocks (events, meshing, frame, tick, camera,
// effects, menus, window loop), startup, the physics view of the world, entity sets, memory counter.
mod app_camera;
mod app_effects;
mod app_entity_sets;
mod app_events;
mod app_frame;
mod app_menus;
mod app_meshing;
mod app_tick;
mod app_window;
mod grid_world;
mod memstats;
mod startup;

// Moved out of this file; other modules still reach them as `crate::...`.
pub(crate) use app_entity_sets::holder_name;
pub(crate) use grid_world::{GridWorld, aim_shape};

use hashbrown::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use rc_client::Session;
use rc_world::ChunkGrid;
use winit::keyboard::KeyCode;

use rc_mesh::blocks::{self, Tables};
use rc_physics::Player;
use render::Renderer;

#[cfg(not(feature = "memstats"))]
#[global_allocator]
static ALLOC: debug_sys::CountingAllocator = debug_sys::CountingAllocator;

/// Logs once the delay until the first frame drawn (startup measurement).
fn first_frame(started: Instant) {
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        eprintln!("first frame: {:.0} ms", started.elapsed().as_secs_f64() * 1e3);
        boot::finish();
    });
}

/// First frame drawn / `startedCleanly` already set back to true (startup safeguard).
static FIRST_DRAWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STARTED_CLEANLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Duration of a game tick (20 per second).
const TICK: f64 = 0.05;

/// Dedicated threads outside the pool: main (window + render), network.
const DEDICATED_THREADS: usize = 2;

/// Values of a `player_position` (position, velocity, yaw, pitch): those of the packet resolved,
/// or the current values unchanged if the player is a passenger.
fn player_position_values(
    p: &rc_proto_v777::play::PlayerPosition,
    passenger: bool,
    (pos, vel, yaw, pitch): ([f64; 3], [f64; 3], f32, f32),
) -> ([f64; 3], [f64; 3], f32, f32) {
    if passenger { (pos, vel, yaw, pitch) } else { resolve_teleport(p, pos, vel, yaw, pitch) }
}

/// Teleport resolved like the game: relative (flags) or absolute position and rotations,
/// velocity rotated by the rotation difference if requested, then relative or absolute per axis.
fn resolve_teleport(
    p: &rc_proto_v777::play::PlayerPosition,
    pos: [f64; 3],
    vel: [f64; 3],
    yaw: f32,
    pitch: f32,
) -> ([f64; 3], [f64; 3], f32, f32) {
    use rc_proto_v777::play::relative as r;
    let has = |f: i32| p.relative & f != 0;
    let new_pos = std::array::from_fn(|i| if has([r::X, r::Y, r::Z][i]) { pos[i] } else { 0.0 } + p.pos[i]);
    let new_yaw = if has(r::YAW) { yaw } else { 0.0 } + p.yaw;
    let new_pitch = (if has(r::PITCH) { pitch } else { 0.0 } + p.pitch).clamp(-90.0, 90.0);
    let mut v = vel;
    if has(r::ROTATE_DELTA) {
        // Rotation around x (pitch difference) then y (yaw difference), table sine.
        let rad = |deg: f32| f64::from(deg).to_radians() as f32;
        let (c, s) = (
            f64::from(rc_physics::cos_t(rad(pitch - new_pitch))),
            f64::from(rc_physics::sin_t(rad(pitch - new_pitch))),
        );
        v = [v[0], v[1] * c + v[2] * s, v[2] * c - v[1] * s];
        let (c, s) =
            (f64::from(rc_physics::cos_t(rad(yaw - new_yaw))), f64::from(rc_physics::sin_t(rad(yaw - new_yaw))));
        v = [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s];
    }
    let new_vel =
        std::array::from_fn(|i| if has([r::DELTA_X, r::DELTA_Y, r::DELTA_Z][i]) { v[i] } else { 0.0 } + p.delta[i]);
    (new_pos, new_vel, new_yaw, new_pitch)
}

/// Pool size: logical cores minus the dedicated threads, `RUSTCRAFT_THREADS` to force.
fn pool_threads() -> usize {
    rc_data::env_str!("RUSTCRAFT_THREADS").and_then(|v| v.parse().ok()).unwrap_or_else(|| {
        std::thread::available_parallelism().map_or(1, |n| n.get()).saturating_sub(DEDICATED_THREADS).max(1)
    })
}

struct World {
    grid: ChunkGrid,
    min_y: i32,
    height: i32,
    state_bits: u8,
    /// Current version of each column (the mesh of a stale version is ignored).
    versions: HashMap<(i32, i32), u64>,
    /// Sections to remesh per column (`remesh.rs`).
    dirty: HashMap<(i32, i32), rc_mesh::wire::Sections>,
    counter: u64,
    meshes: remesh::Meshes,
    /// `rc_world::heightmap::HM_*` bits per state, from the received tags: height maps
    /// followed at each block change (empty before the tags).
    hm_rules: Vec<u8>,
}

impl World {
    fn touch(&mut self, x: i32, z: i32) {
        self.touch_sections(x, z, rc_mesh::wire::Sections::ALL);
    }

    fn touch_with_neighbors(&mut self, x: i32, z: i32) {
        self.touch(x, z);
        for (dx, dz) in rc_mesh::wire::NEIGHBORS {
            if self.grid.get(x + dx, z + dz).is_some() {
                self.touch(x + dx, z + dz);
            }
        }
    }
}

struct App {
    session: Session,
    tables: Arc<Tables>,
    models: Arc<rc_assets::BlockModels>,
    /// Images of the block textures, emptied once sent to the GPU.
    textures: Vec<Vec<rc_assets::texture::Image>>,
    /// Images of the sky, emptied once sent to the GPU.
    sky_assets: sky::SkyAssets,
    /// Title screen panorama, only with `RUSTCRAFT_PANORAMA=1` (test).
    panorama_assets: Option<panorama::PanoramaAssets>,
    /// Interface (atlas, fonts, translations), handed over to the renderer when it is created.
    gui_assets: Option<gui::GuiAssets>,
    entity_textures: Option<entity_render::EntityTextures>,
    entities: entities::Entities,
    /// Block entities drawn (`block-entities.md`).
    block_entities: block_entities::BlockEntities,
    /// Technical blocks: structure, jigsaw, test (`tech_blocks.rs`).
    tech: tech_blocks::TechBlocks,
    /// Local replay of the piston events (`piston_replay.rs`).
    piston_replay: rc_sim::PistonReplay,
    /// Moving blocks to advance after the local player's tick (`piston_replay.rs`).
    pistons_due: bool,
    particles: particles::Particles,
    players: players::PlayerList,
    skins: skins::Skins,
    /// UUID of the local player (offline profile).
    local_uuid: u128,
    tab_list: tablist::TabList,
    scores: scoreboard::Scoreboard,
    /// Keys of the "below name" lines kept between frames (`app_entity_sets.rs`).
    below_line_keys: app_entity_sets::BelowLineKeys,
    /// Audio engine, joined at first use (`audio_slot.rs`); no sound with `RUSTCRAFT_NOAUDIO`.
    audio: audio_slot::Audio,
    /// Sounds attached to an entity: (sound, entity), position copied at each tick.
    entity_sounds: Vec<(rc_audio::SoundHandle, i32)>,
    world_random: level_events::WorldRandom,
    /// Music, ambience and steps (sounds without a packet).
    world_sounds: world_sounds::WorldSounds,
    /// Profile name of the local player.
    local_name: String,
    hud_extra: hud_extra::HudExtra,
    /// Server in online mode (`login` connection).
    online_mode: bool,
    pending_registries: Vec<(String, rc_client::RegistryEntries)>,
    /// Registries and tags received for the creative inventory.
    creative_rec: creative::Received,
    creative_ui: creative_screen::CreativeScreen,
    hotbars: hotbars::Hotbars,
    recipe_book: recipe_book::RecipeBook,
    toasts: toasts::Toasts,
    /// World effects, jukebox, music notification (`world_fx.rs`).
    fx: world_fx::WorldFx,
    /// Local effects of the entity events, totem animation (`entity_events.rs`).
    ev: entity_events::EventFx,
    /// Random animation of the blocks and fluids (`block_anim.rs`).
    block_anim: block_anim::BlockAnim,
    /// Resource packs: stack, reload, server packs (`reload.rs`).
    packs: reload::Packs,
    /// Advancements and statistics (`progress_app.rs`).
    progress: progress_app::Progression,
    chat: chat::Chat,
    /// Chat commands: tree, parsing, suggestions (`commands.md`).
    cmd: commands::CommandUi,
    hud: hud::Hud,
    /// Item cooldowns received (`cooldown`), per group.
    cooldowns: rc_client::cooldowns::Cooldowns,
    inventory: inventory::PlayerInventory,
    component_ids: stack::ComponentIds,
    /// Inventory and container screens.
    screens: screens::Screens,
    /// Modifier keys (clicks of the screens).
    mods: winit::keyboard::ModifiersState,
    /// Keyboard-free demonstration (`RUSTCRAFT_INVTEST`): step, ticks elapsed.
    invtest: (usize, u32),
    /// Interactions with the world: mining, placing, attack, use (`interact.rs`).
    act: interact::Interact,
    item_names: Vec<String>,
    dimension: String,
    /// Spawn point of the world (dimension, block).
    spawn: Option<(String, [i32; 3])>,
    /// Last death location of the local player (spawn info of the connection and of the
    /// respawns): target of the recovery compass.
    last_death: Option<(String, [i32; 3])>,
    /// Sun angle at the last tick (hand of the clocks).
    sun_angle: f32,
    /// Death screen open, and the server's choice (rule "immediate respawn").
    death: Option<death::DeathScreen>,
    show_death_screen: bool,
    /// End screen (`win_screen.rs`), spectator menu, bed (`bed.rs`).
    win: Option<win_screen::WinScreen>,
    spectator: spectator::SpectatorUi,
    bed: bed::Bed,
    /// Cursor in physical pixels of the window.
    cursor: Option<(f64, f64)>,
    /// Directional shading of the current dimension.
    shade: [f32; 6],
    /// Video settings applied outside the render: frame limiter, mesh options (`video.rs`).
    video: video::VideoState,
    /// Playback position of each texture animation (`models.animations`).
    clocks: Vec<rc_assets::AnimationClock>,
    data: Arc<rc_data::VersionData>,
    colormaps: rc_assets::texture::Colormaps,
    palette: Arc<blocks::BiomePalette>,
    /// Visual environment: attributes, clocks, weather, fog.
    env: environ::EnvState,
    /// Biomes of the current world, in network order.
    biome_names: Vec<String>,
    world: World,
    renderer: Option<Renderer>,
    /// GPU being opened in the background until the window exists (`gpu_backend::EarlyGpuOpen`).
    early_gpu: Option<gpu_backend::EarlyGpuOpen>,
    /// First session event that needed the renderer while it was being built: applied first at the
    /// next frame (`events_before_renderer.rs`).
    held_event: Option<rc_client::Event>,
    meshes_tx: Sender<remesh::Done>,
    meshes_rx: Receiver<remesh::Done>,
    /// Meshes received not yet uploaded (upload budget per frame, `RUSTCRAFT_OPT_UPLOADBUDGET`).
    mesh_pending: Vec<remesh::Done>,
    in_flight: usize,
    /// What the last pick that held every dirty column saw (`app_meshing::HeldSignature`).
    all_held: Option<app_meshing::HeldSignature>,
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
    placed: bool,
    keys: HashSet<KeyCode>,
    /// Rebindable keys and state of the actions (`keys.rs`).
    binds: keys::Binds,
    grabbed: bool,
    last: Instant,
    fps_frames: u32,
    fps_since: Instant,
    /// Title refreshed every second, in a world and outside (`window_title.rs`).
    window_title: window_title::WindowTitle,
    started: Instant,
    status: String,
    /// Fixed game tick at 20 per second: accumulated time not yet consumed.
    tick_acc: f64,
    /// Singleplayer pause (`pause.rs`): frozen tick fraction.
    pause: ticking::Pause,
    sync: rc_client::sync::PositionSync,
    /// Local player (faithful physics); positioned by the first position packet.
    player: Player,
    positioned: bool,
    /// Position at the previous tick, to interpolate the camera between two ticks.
    prev_pos: [f64; 3],
    phys: rc_physics::Tables,
    /// Client light engine and positions to recompute before the next meshing.
    light: light_engine::LightTables,
    relight: Vec<[i32; 3]>,
    /// Those of `relight` that touch the sky (`RUSTCRAFT_OPT_SKYBIT`).
    relight_sky: Vec<[i32; 3]>,
    /// Singleplayer: sections whose light came from the server since the last local relight
    /// (`RUSTCRAFT_OPT_SOLONORELIGHT`).
    light_recv: light_engine::Received,
    /// `RUSTCRAFT_SOLOLIGHTCHECK`: server light kept for comparison.
    light_shadow: Option<Box<light_engine::Shadow>>,
    player_id: i32,
    /// Names of the attributes in the order of their network id.
    attr_names: Vec<String>,
    last_input: Option<u8>,
    /// Free camera (F) instead of the player's view.
    free_cam: bool,
    autowalk: Option<f64>,
    autowalk_ticks: u32,
    /// Per-tick trace (`RUSTCRAFT_TRACE=file`): position, velocity, ground, and received teleports.
    trace: Option<std::io::BufWriter<std::fs::File>>,
    /// Camera (perspective, field of view, sways) and position of the current frame.
    camera: camera::Camera,
    shot: camera::Shot,
    /// Singleplayer quick play waiting for the first out-of-world frame (requested world), and
    /// launcher game log (`quickplay.rs`).
    quick_play: Option<String>,
    quick_log: quickplay::Log,
    /// Capture requested by `RUSTCRAFT_CAPTURE=file.bmp;seconds`, then closing.
    capture: Option<(std::path::PathBuf, f64)>,
    quit_after_capture: bool,
    /// Resources (translations reloaded when the language changes).
    assets: Arc<rc_assets::Assets>,
    /// Player settings, `options.txt` file, last client information sent.
    options: options::Options,
    options_path: std::path::PathBuf,
    last_info: rc_proto_v777::play::ClientInformation,
    /// Pause menu and options screens (`option-menus.md`).
    menus: menus::Menus,
    /// Window without focus since (pause after 500 ms).
    unfocused_at: Option<Instant>,
    /// Closing requested (disconnection): done at the next pass of the loop.
    exit_requested: bool,
    /// Integrated server of the singleplayer (`rc-sim`), world open.
    solo: Option<rc_sim::Handle>,
    /// Keyboard-free menu demonstration (`RUSTCRAFT_MENUTEST`): step, ticks, simulated mouse.
    menutest: (usize, u32, Option<(f32, f32)>),
    /// Weather and world border (`weather.rs`).
    weather: weather::WeatherState,
    /// Out-of-world screens (title screen, servers, connection) and application state (`flow.rs`).
    front: front::Front,
    /// F3 debug screen (`debug-f3.md`).
    debug: debug_screen::DebugScreen,
    /// Exact-fidelity packets (`exact.rs`).
    exact: exact::Exact,
    /// Mounts and vehicles (`riding.rs`).
    riding: riding::Riding,
    /// Leashes, pickup, screen effects, subtitles, isolated packets (`misc.rs`).
    misc: misc::Misc,
    /// Special screens outside containers, 3D interface models (`special.rs`).
    special: special::Special,
}

const DIGITS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

fn main() {
    startup::run();
}

#[cfg(test)]
mod tests {
    use super::{player_position_values, resolve_teleport};
    use rc_proto_v777::play::{PlayerPosition, relative};

    #[test]
    fn relative_teleport_resolves_against_current_state() {
        let p = PlayerPosition {
            id: 1,
            pos: [1.0, 0.0, -2.0],
            delta: [0.5, 0.0, 0.0],
            yaw: 10.0,
            pitch: 100.0,
            relative: relative::X | relative::Z | relative::YAW | relative::DELTA_Y,
        };
        let (pos, vel, yaw, pitch) = resolve_teleport(&p, [10.0, 64.0, 10.0], [0.1, 0.2, 0.3], 5.0, 0.0);
        assert_eq!(pos, [11.0, 0.0, 8.0]);
        assert_eq!(vel, [0.5, 0.2, 0.0]);
        assert_eq!((yaw, pitch), (15.0, 90.0));
    }

    /// Passenger: `player_position` ignored, acknowledged with the current values.
    #[test]
    fn passenger_ignores_player_position() {
        let p = rc_proto_v777::play::PlayerPosition {
            id: 3,
            pos: [100.0, 70.0, -5.0],
            delta: [0.0; 3],
            yaw: 90.0,
            pitch: 10.0,
            relative: 0,
        };
        let cur = ([1.0, 64.0, 2.0], [0.1, 0.0, 0.0], 5.0, -3.0);
        assert_eq!(player_position_values(&p, true, cur), cur);
        assert_eq!(player_position_values(&p, false, cur).0, [100.0, 70.0, -5.0]);
    }
}
