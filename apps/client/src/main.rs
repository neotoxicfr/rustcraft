//! Rustcraft, milestone 1: offline connection, receiving the world, wireframe display,
//! free camera (ZQSD/WASD, space/shift, mouse after a click, Esc to release).
//!
//! `rustcraft [--username name]`: title screen; `rustcraft host [port] [name]`: direct
//! connection (port 25565, name Rustcraft by default).

mod alloc_trace;
mod arena;
mod audio_slot;
mod block_anim;
mod boot;
mod block_entities;
mod block_fx;
mod block_text;
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
mod exact;
mod explode;
mod flow;
mod front;
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
mod text_display;
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

use hashbrown::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use rc_client::{Config, Event, Session};
use rc_world::{ChunkGrid, Column};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, WindowId};

use rc_mesh::blocks::{self, Tables};
use rc_mesh::wire::MeshJob;
use rc_physics::{Input, Player};
use render::Renderer;

#[cfg(not(feature = "memstats"))]
#[global_allocator]
static ALLOC: debug_sys::Gated = debug_sys::Gated;

/// Memory measurement (`--features memstats`): live bytes on our heap, counted around
/// mimalloc. Without this option, no cost.
#[cfg(feature = "memstats")]
#[allow(unsafe_code)]
mod memstats {
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            LIVE.fetch_add(l.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.alloc(l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            LIVE.fetch_add(l.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator (fresh pages already zero, as without
            // the measurement: private memory stays comparable).
            unsafe { mimalloc::MiMalloc.alloc_zeroed(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            // SAFETY: `p` comes from `alloc` with the same layout.
            unsafe { mimalloc::MiMalloc.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            LIVE.fetch_add(n, Ordering::Relaxed);
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static ALLOC: Counting = Counting;

    /// Live heap after a startup step.
    pub fn step(what: &str) {
        eprintln!("heap after {what}: {:.1} MB", LIVE.load(Ordering::Relaxed) as f64 / 1_048_576.0);
    }
}

#[cfg(not(feature = "memstats"))]
mod memstats {
    pub fn step(_: &str) {}
}

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
    meshes_tx: Sender<remesh::Done>,
    meshes_rx: Receiver<remesh::Done>,
    /// Meshes received not yet uploaded (upload budget per frame, `RUSTCRAFT_OPT_UPLOADBUDGET`).
    mesh_pending: Vec<remesh::Done>,
    in_flight: usize,
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

impl App {
    fn apply(&mut self, ev: Event) {
        let air = |s: u32, t: &Tables| t.air.get(s as usize).copied().unwrap_or(true);
        match ev {
            Event::Joined { login, height, biomes, dimension, flat } => {
                self.player_id = login.player_id;
                self.debug.logs.subscribed = false; // F3+2 subscriptions sent back to the server
                self.unsecure_toast(login.enforces_secure_chat);
                self.cmd.data.dimensions = login.dimensions.clone();
                self.entities.local = login.player_id;
                self.entities.clear();
                self.misc.reset_world();
                self.progress.join();
                // Local player, drawn in third person (`hand-camera.md` § 3.3).
                let kind =
                    self.entities.type_names.iter().position(|n| n == "minecraft:player").map_or(-1, |i| i as i32);
                self.entities.spawn_me(self.local_uuid, kind, self.player.pos, self.yaw);
                self.particles.clear();
                self.ev.totem.reset();
                self.act.reset_world();
                self.palette = Arc::new(blocks::biome_palette(&self.data, &self.colormaps, &biomes));
                self.env.join(&self.data, &dimension, &biomes, flat);
                crate::environ::set_biome_zoom_seed(login.spawn.hashed_seed);
                self.world_sounds.join(self.audio.get());
                self.dimension = dimension.clone();
                self.env.server_view = login.view_distance;
                self.env.game_mode = login.spawn.game_mode;
                self.last_death = login.spawn.last_death.as_ref().map(|(d, p)| (d.clone(), [p.x, p.y, p.z]));
                // Quick play log (`--quickPlayPath`), at each entry into the game.
                let lang = self.renderer.as_ref().map(|r| &r.gui.lang);
                self.quick_log.log(login.spawn.game_mode, || {
                    lang.map_or_else(|| "Minecraft Server".into(), |l| widgets::tr(l, "selectServer.defaultName", &[]))
                });
                self.hud.hardcore = login.hardcore;
                self.online_mode = login.online_mode;
                self.debug.joined(login.simulation_distance, login.reduced_debug_info);
                self.hud_extra.reset();
                self.player.effects = Default::default();
                self.hud.more.clear_waypoints();
                maps::store().clear();
                self.scores = scoreboard::Scoreboard::default();
                if self.hud_extra.effect_names.is_empty() {
                    self.hud_extra.effect_names =
                        self.data.registry("minecraft:mob_effect").map(|r| r.entries.clone()).unwrap_or_default();
                    self.hud_extra.effect_colors = self.data.effect_colors.clone();
                }
                self.show_death_screen = login.show_death_screen;
                self.death = None;
                (self.win, self.world_sounds.screen_music) = (None, None);
                self.block_anim.set_biomes(&biomes);
                self.biome_names = biomes;
                self.world.grid = level_grid(login.view_distance);
                self.block_entities.clear();
                self.tech.clear();
                self.world.min_y = height.min_y;
                self.world.height = height.height;
                let sea = Some(login.spawn.sea_level);
                self.weather.join(&self.data, &self.biome_names, sea, height.min_y, height.height);
                self.set_shade(height.nether_light);
                if let Some(r) = &self.renderer
                    && let Some(e) = &r.entities
                {
                    e.set_dimension(&r.queue_handle(), height.nether_light);
                }
                self.world.versions.clear();
                self.world.meshes.clear();
                if let Some(r) = &mut self.renderer {
                    r.clear();
                }
            }
            Event::Respawn { height, dimension, flat, keep, last_death } => {
                self.last_death = last_death;
                self.hud.respawn(keep & 2 != 0);
                // New player: no effect until the server sends some back.
                self.player.effects = Default::default();
                // New player: player-specific states reset to zero except retention of the entity
                // data (flag 2).
                if keep & 2 == 0 {
                    self.player.reset_entity_state();
                }
                self.entities.clear();
                self.entities.respawn_me(keep & 2 != 0, self.player.pos, self.yaw);
                self.misc.reset_world();
                self.particles.clear();
                self.ev.totem.reset();
                self.act.reset_world();
                if self.death.take().is_some() && !self.front.is_open() {
                    self.grab(true);
                }
                let names = std::mem::take(&mut self.biome_names);
                self.env.join(&self.data, &dimension, &names, flat);
                self.biome_names = names;
                self.world_sounds.respawn(self.audio.get(), dimension != self.dimension);
                // Current dimension (music, compass, thunderstorm): remained that of the connection.
                self.dimension = dimension;
                // New client world at the server's view radius (that of the connection or of the
                // last `set_chunk_cache_radius`), like the game: a fixed radius of 16 made the columns
                // of a wider server view overlap.
                self.world.grid = level_grid(self.env.server_view);
                self.block_entities.clear();
                self.tech.clear();
                self.world.min_y = height.min_y;
                self.world.height = height.height;
                self.weather.join(&self.data, &self.biome_names, None, height.min_y, height.height);
                self.set_shade(height.nether_light);
                if let Some(r) = &self.renderer
                    && let Some(e) = &r.entities
                {
                    e.set_dimension(&r.queue_handle(), height.nether_light);
                }
                self.world.versions.clear();
                self.world.meshes.clear();
                if let Some(r) = &mut self.renderer {
                    r.clear();
                }
            }
            Event::ViewDistance(d) if d > 0 => {
                self.world.grid.set_radius(d as u32);
                self.env.server_view = d;
            }
            Event::Chat { component, tag } => self.chat.add(component, tag),
            Event::ChatSignature(sig) => self.chat.attach_signature(sig),
            Event::ChatOriginal(text) => self.chat.attach_original(text),
            Event::DeleteChat(sig) => self.chat.delete(sig),
            Event::Commands(c) => self.cmd.set_tree(&c),
            Event::CommandSuggestions(s) => self.op_suggestions(s),
            Event::ChatCompletions { action, entries } => self.cmd.set_completions(action, entries),
            Event::PlayerInfo(u) => {
                self.players.apply(u);
                // `RUSTCRAFT_SKINTEST=<name>`: public signed profile of that name applied to the
                // other players without a skin (test of the download and of the signature).
                if let Some(name) = rc_data::env_str!("RUSTCRAFT_SKINTEST") {
                    skins::apply_test_profile(&mut self.players, name, self.local_uuid);
                }
                self.skins.update(&self.players, self.local_uuid);
            }
            Event::PlayersRemoved(u) => self.players.remove(&u),
            Event::TabList { header, footer } => (self.tab_list.header, self.tab_list.footer) = (header, footer),
            Event::Health { health, food, saturation } => self.hud.set_health(health, food, saturation),
            Event::Cooldown { group, ticks } => self.cooldowns.apply(&group, ticks),
            Event::Experience { progress, level, .. } => self.hud.set_experience(progress, level),
            Event::HeldSlot(s) => self.hud.held_slot(s),
            Event::SpawnPosition { dimension, pos } => self.spawn = Some((dimension, pos)),
            // Applied to the render at the next frame (it may not exist yet).
            Event::Registry { name, entries } => {
                if name == "minecraft:test_instance" {
                    self.set_test_registry(&entries);
                }
                if name == "minecraft:damage_type" {
                    self.ev_damage_registry(&entries);
                }
                if name == "minecraft:instrument" {
                    self.act.instruments = interact::instrument::table(&entries, |p| self.assets.read(p));
                }
                self.cmd.registry(&name, &entries);
                self.creative_rec.registry(&name, &entries);
                tooltips_items::set_registry(&name, &entries);
                item_regs::set_registry(&name, &entries);
                if name == "minecraft:enchantment" {
                    self.screens.hasher.enchantments = entries.into_iter().map(|e| e.0).collect();
                } else if name == "minecraft:banner_pattern" {
                    screens_work::set_patterns(&entries);
                } else if name == "minecraft:jukebox_song" {
                    self.fx.set_songs(&entries);
                } else {
                    self.packs.remember_registry(&name, &entries);
                    self.pending_registries.push((name, entries));
                }
            }
            Event::OpenScreen { window, kind, title } => {
                self.inventory.open(window, menu::Kind::from_registry(kind), title);
                self.screens.show();
                self.keys.clear();
                self.grab(false);
            }
            Event::ContainerClosed => {
                self.inventory.container = None;
                if self.screens.open {
                    self.screens.hide();
                    self.grab(true);
                }
            }
            ev @ (Event::MerchantOffers(_)
            | Event::UpdateRecipes(_)
            | Event::MountScreen { .. }
            | Event::OpenBook(_)
            | Event::OpenSignEditor { .. }) => self.special_apply(ev),
            Event::TagQuery { id, nbt } => self.tag_query(id, nbt),
            Event::TestInstanceStatus { status, size } => self.op_test_status(status, size),
            Event::GameTestHighlight { absolute, relative } => {
                self.tech.highlight(absolute, relative, std::time::Instant::now());
            }
            Event::ContainerData { window, prop, value } => {
                if rc_data::env_flag!("RUSTCRAFT_INVLOG") {
                    eprintln!("property: window {window}, {prop} = {value}");
                }
                self.inventory.set_data(window, prop, value)
            }
            Event::Score(s) => self.scores.apply(s),
            Event::Sound(s) => self.play_sound(s),
            Event::Particles(p) => self.level_particles(&p),
            Event::LevelEvent { kind, pos, data, global } => self.on_level_event(kind, pos, data, global),
            Event::StopSound { category, name } => {
                if let Some(a) = self.audio.get() {
                    a.stop(category.and_then(rc_audio::Category::from_id), name.as_deref());
                }
            }
            Event::Boss(b) => self.hud_extra.boss_event(b),
            Event::Title(t) => self.hud_extra.set_title(t),
            Event::Subtitle(t) => self.hud_extra.set_subtitle(t),
            Event::TitleTimes(t) => self.hud_extra.set_times(t),
            Event::ClearTitles(reset) => self.hud_extra.clear_titles(reset),
            Event::Entity(e) => {
                if rc_data::env_flag!("RUSTCRAFT_ENTLOG") {
                    eprintln!("entity: {e:?}");
                }
                self.bed_entity_event(&e);
                self.m6_entity_event(&e);
                self.entity_event_fx(&e);
                // Effects of the local player: HUD (`hud-advanced.md` § 2).
                match e {
                    rc_client::EntityEvent::Effect { id, effect, amplifier, duration, flags }
                        if id == self.player_id =>
                    {
                        self.hud_extra.effect(effect, amplifier, duration, flags);
                        let name = self.hud_extra.effect_names.get(effect as usize).map(String::as_str);
                        self.video.darkness.effect(name, duration, flags);
                        self.hud.more.effect(name, duration, flags);
                        self.player.effects.set(name.unwrap_or(""), Some(amplifier));
                    }
                    rc_client::EntityEvent::RemoveEffect { id, effect } if id == self.player_id => {
                        self.hud_extra.remove_effect(effect);
                        self.video.darkness.remove(self.hud_extra.effect_names.get(effect as usize).map(String::as_str));
                        self.hud
                            .more
                            .remove_effect(self.hud_extra.effect_names.get(effect as usize).map(String::as_str));
                        if let Some(n) = self.hud_extra.effect_names.get(effect as usize) {
                            self.player.effects.set(n, None);
                        }
                    }
                    // Reduced info (22, 23) and permission level (24 to 28): F3 screen.
                    rc_client::EntityEvent::Event { id, event } if id == self.player_id => {
                        self.debug.player_event(i32::from(event));
                        if (24..=28).contains(&event) {
                            self.cmd.data.op_level = i32::from(event) - 24;
                        }
                    }
                    // Knockback (hits, arrows, wind charges…): velocity of the local player replaced.
                    rc_client::EntityEvent::Motion { id, velocity } if id == self.player_id => {
                        self.player.set_motion(velocity);
                    }
                    // Glide and spin attack as the server sends them back.
                    rc_client::EntityEvent::Data { id, ref values } if id == self.player_id => {
                        player_env::local_flags(values, &mut self.player);
                    }
                    _ => {}
                }
                // Mount removed from under the player: its teleport applies to the player (without acknowledgement).
                if let Some(t) = self.entities.track_player_vehicle(&e, self.player_id) {
                    let p = rc_proto_v777::play::PlayerPosition {
                        id: t.id,
                        pos: t.pos,
                        delta: t.velocity,
                        yaw: t.yaw,
                        pitch: t.pitch,
                        relative: t.relative,
                    };
                    let (pos, vel, yaw, pitch) =
                        resolve_teleport(&p, self.player.pos, self.player.vel, self.yaw, self.pitch);
                    (self.player.pos, self.player.vel, self.prev_pos) = (pos, vel, pos);
                    (self.yaw, self.pitch) = (yaw, pitch);
                }
                // Locally piloted vehicle: only `move_vehicle` moves it (`riding.rs`).
                if self.piloted_move(&e) {
                    return;
                }
                let added = if let rc_client::EntityEvent::Add(a) = &e { Some(a.id) } else { None };
                self.entities.apply(e);
                // Sustained sounds created at spawn (minecarts, bees: `entity_loops.rs`).
                if let Some(id) = added {
                    self.loop_sound_added(id);
                }
            }
            Event::Inventory(inv) => {
                if rc_data::env_flag!("RUSTCRAFT_INVLOG") {
                    eprintln!("inventory: {inv:?}");
                }
                self.inventory.apply(inv);
            }
            Event::Died { entity, message } if entity == self.player_id => self.open_death(message),
            Event::ActionBar(c) => self.chat.action_bar(c),
            Event::LightUpdate { x, z, light } => {
                // Check of the client engine: differences between our light and the server's.
                if rc_data::env_flag!("RUSTCRAFT_LIGHTCHECK")
                    && let Some(col) = self.world.grid.get(x, z)
                {
                    let diff = |ours: &[rc_world::LightLayer], theirs: &[rc_world::LightLayer]| {
                        ours.iter()
                            .zip(theirs)
                            .filter_map(|(a, b)| Some((a.as_ref()?, b.as_ref()?)))
                            .map(|(a, b)| {
                                (0..rc_world::LIGHT_BYTES * 2)
                                    .filter(|&i| (a[i >> 1] >> ((i & 1) * 4)) & 15 != (b[i >> 1] >> ((i & 1) * 4)) & 15)
                                    .count()
                            })
                            .sum::<usize>()
                    };
                    eprintln!(
                        "light ({x}, {z}): differences block {}, sky {}",
                        diff(&col.block_light, &light.block),
                        diff(&col.sky_light, &light.sky)
                    );
                }
                let o = light_engine::opts();
                if o.solo_cut && self.solo.is_some() {
                    self.light_recv.note(x, z, &light.sky, &light.block);
                }
                if let Some(s) = &mut self.light_shadow {
                    s.update(x, z, &light.sky, &light.block);
                }
                // Sections where a light cell changes (compared before the replacement).
                // Nothing changed (`LIGHTNOCOPY`): the column, perhaps held by a meshing task,
                // is not copied to put the same layers back there.
                let changed = self.world.touch_light(x, z, &light.sky, &light.block, o.no_copy);
                if (changed || !o.no_copy)
                    && let Some(col) = self.world.grid.get_mut(x, z)
                {
                    for (dst, src) in [(&mut col.sky_light, light.sky), (&mut col.block_light, light.block)] {
                        for (d, s) in dst.iter_mut().zip(src) {
                            if s.is_some() {
                                *d = s;
                            }
                        }
                    }
                    self.weather.rev += 1;
                }
            }
            Event::Border(p) => self.weather.on_border(p),
            Event::Time { game_time, clocks } => self.env.env.clocks.set_time(game_time, &clocks),
            Event::GameEvent { kind, param } => {
                self.env.env.weather.event(kind, param);
                // Game mode change.
                if kind == 3 {
                    self.env.game_mode = param as i32;
                }
                // Rule "immediate respawn".
                if kind == 11 {
                    self.show_death_screen = param == 0.0;
                }
                self.m5_game_event(kind, param);
            }
            Event::Chunk(c) => {
                let c = *c;
                let (x, z) = (c.x, c.z);
                let mut col = Column {
                    x,
                    z,
                    sections: c.sections.into_boxed_slice(),
                    sky_light: c.light.sky.into_boxed_slice(),
                    block_light: c.light.block.into_boxed_slice(),
                    heightmaps: c.heightmaps,
                };
                if let Some(old) = self.world.grid.get(x, z) {
                    col.inherit_heightmaps(old);
                }
                col.complete_heightmaps(&self.world.hm_rules, self.world.height as usize);
                if let Some(s) = &mut self.light_shadow {
                    s.chunk(x, z, &col.sky_light, &col.block_light);
                }
                self.world.grid.insert(col);
                let view =
                    environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
                self.block_entities.load_chunk(x, z, &c.block_entities, |x, y, z| view.state(x, y, z));
                self.tech.load_chunk(x, z, &c.block_entities);
                self.world.touch_with_neighbors(x, z);
                if let Some(col) = self.world.grid.get(x, z) {
                    let b = weather::Blocking { data: &self.data, phys: &self.phys };
                    self.weather.on_chunk(col, &b);
                }
            }
            Event::BlockEvent { pos, b0, b1, .. } => {
                self.piston_event(pos, b0, b1);
                self.block_entities.on_event(pos, b0, b1, self.env.env.clocks.game_time);
            }
            Event::BlockEntityData { pos, kind, nbt } => {
                self.op_block_data(pos, nbt.as_ref());
                self.tech.on_data(pos, kind, nbt.as_ref());
                self.block_entities.on_data(pos, kind, nbt.as_ref())
            }
            Event::ForgetChunk { x, z } => {
                self.block_entities.forget_chunk(x, z);
                self.tech.forget_chunk(x, z);
                self.world.grid.remove(x, z);
                if let Some(s) = &mut self.light_shadow {
                    s.forget(x, z);
                }
                self.weather.forget(x, z);
                self.world.versions.remove(&(x, z));
                self.world.meshes.forget((x, z));
                if let Some(r) = &mut self.renderer {
                    r.remove(x, z);
                }
            }
            // Blocks awaiting a prediction: the received state is only stored (§ 0.1).
            Event::BlockUpdate { pos, state } if self.act.server_block([pos.x, pos.y, pos.z], state) => {}
            Event::BlockChangedAck(seq) => self.act_ack(seq),
            Event::BlockDestruction { id, pos, stage } => self.act.crack(id, pos, i32::from(stage)),
            Event::BlockUpdate { pos, state } => {
                let (cx, cz) = (pos.x >> 4, pos.z >> 4);
                let sy = (pos.y - self.world.min_y) >> 4;
                let idx = rc_world::block_index((pos.x & 15) as usize, (pos.y & 15) as usize, (pos.z & 15) as usize);
                let relight = self.server_relight(cx, sy, cz);
                self.update_section_lit(cx, cz, sy, &[(idx as u16, state)], air, relight);
            }
            Event::SectionUpdate { section, changes } => {
                let sy = section[1] - (self.world.min_y >> 4);
                let changes = self.act.filter_section(section, changes);
                let relight = self.server_relight(section[0], sy, section[2]);
                self.update_section_lit(section[0], section[2], sy, &changes, air, relight);
            }
            Event::Teleport(p) => {
                // Passenger: position ignored, acknowledged with the current position (like the game).
                let passenger = self.vehicle().is_some();
                let current = (self.player.pos, self.player.vel, self.yaw, self.pitch);
                let (pos, vel, yaw, pitch) = player_position_values(&p, passenger, current);
                if let Some(t) = &mut self.trace {
                    use std::io::Write;
                    let _ = writeln!(t, "tp {:.4} {:.4} {:.4}", pos[0], pos[1], pos[2]);
                }
                if !passenger {
                    self.player.pos = pos;
                    self.player.vel = vel;
                    self.prev_pos = pos;
                }
                let _ = self.session.link.send_accept_teleport(p.id, pos, yaw, pitch);
                self.act.teleported();
                self.stop_destroy();
                self.positioned = true;
                (self.yaw, self.pitch) = (yaw, pitch);
                if !self.placed {
                    self.eye = [pos[0], pos[1] + self.player.eye_height(), pos[2]];
                    self.placed = true;
                }
            }
            Event::Abilities(a) => {
                self.player.may_fly = a.may_fly;
                self.player.flying = a.flying;
                self.player.fly_speed = a.flying_speed;
                self.world_sounds.instabuild = a.instabuild;
                self.act.invulnerable = a.invulnerable;
                self.camera.walking_speed = a.walking_speed;
            }
            Event::Attributes { entity, attributes } if entity == self.player_id => {
                for a in attributes {
                    if let Some(name) = self.attr_names.get(a.id as usize) {
                        let mods = a.modifiers.into_iter().map(|m| (m.id, m.amount, m.operation)).collect();
                        self.player.set_attribute(name, a.base, mods);
                    }
                }
            }
            Event::Attributes { entity, attributes } => {
                self.riding.attributes(entity, &attributes, &self.attr_names);
                self.hud.more.attributes(entity, &attributes, &self.attr_names);
                // Render scale of living entities, distances of the name tags.
                if let Some(e) = self.entities.map.get_mut(&entity) {
                    for a in &attributes {
                        let Some(name) = self.attr_names.get(a.id as usize) else { continue };
                        match name.as_str() {
                            "minecraft:scale" => e.scale = hud_more::attribute_value(name, a) as f32,
                            "minecraft:name_tag_distance" => e.tag_dist = hud_more::attribute_value(name, a),
                            "minecraft:below_name_distance" => e.below_dist = hud_more::attribute_value(name, a),
                            _ => {}
                        }
                    }
                }
            }
            Event::Waypoint(w) => self.hud.more.waypoint(w),
            Event::MapData(m) => maps::store().apply(m),
            Event::Tags(tags) => {
                self.menus.world.set_tags(&tags);
                self.debug.set_tags(&tags);
                self.binds.set_tags(&tags);
                if let Some((_, list)) = tags.iter().find(|(r, _)| r == "minecraft:dialog" || r == "dialog") {
                    self.exact.dialog_tags = list.clone();
                }
                self.cmd.tags(&tags);
                self.creative_rec.tags(&tags);
                menu_special::set_tags(&tags);
                tooltips::set_tags(&tags);
                tooltips_items::set_block_tags(&tags);
                if let Some((_, block_tags)) = tags.iter().find(|(r, _)| r == "minecraft:block") {
                    self.phys.set_block_tags(block_tags);
                    self.tables.fluid.flow.set_block_tags(block_tags);
                    self.world_sounds.set_block_tags(self.data.blocks.len(), block_tags);
                    self.weather.heights.set_block_tags(self.data.blocks.len(), block_tags);
                    let mut by_name: hashbrown::HashMap<String, Vec<u32>> = block_tags
                        .iter()
                        .map(|(n, ids)| (n.clone(), ids.iter().filter_map(|&i| u32::try_from(i).ok()).collect()))
                        .collect();
                    by_name.values_mut().for_each(|v| v.sort_unstable());
                    self.world.hm_rules = rc_sim::chunk::heightmap_rules(&self.data, &by_name);
                    self.act.tags.set(block_tags);
                    self.block_entities.set_block_tags(block_tags);
                    self.block_anim.set_block_tags(block_tags);
                }
            }
            Event::Disconnected(reason) => {
                eprintln!("disconnected: {reason}");
                self.status = format!("disconnected: {reason}");
            }
            Event::Misc(d) => self.apply_misc(d),
            Event::Progress(p) => self.on_progress(p),
            Event::Recipes(e) => self.on_recipes(e),
            ev => self.apply_exact(ev),
        }
    }

    /// Directional shading of the dimension (cardinal lighting), for the mesher and the shader.
    fn set_shade(&mut self, nether: bool) {
        self.shade = if nether { rc_mesh::wire::SHADE_NETHER } else { rc_mesh::wire::SHADE_DEFAULT };
        if let Some(r) = &mut self.renderer {
            r.shade = self.shade;
        }
    }

    /// Change made by the client itself (prediction, acknowledgement, piston replay): local
    /// light recompute always.
    fn update_section(
        &mut self,
        cx: i32,
        cz: i32,
        sy: i32,
        changes: &[(u16, u32)],
        air: impl Fn(u32, &Tables) -> bool,
    ) {
        self.update_section_lit(cx, cz, sy, changes, air, true);
    }

    /// Change received from the server: local light recompute unless the integrated server has just
    /// sent the section's ([`light_engine::Opts::solo_cut`]).
    fn server_relight(&self, cx: i32, sy: i32, cz: i32) -> bool {
        !(light_engine::opts().solo_cut && self.solo.is_some() && self.light_recv.covers(cx, sy, cz))
    }

    fn update_section_lit(
        &mut self,
        cx: i32,
        cz: i32,
        sy: i32,
        changes: &[(u16, u32)],
        air: impl Fn(u32, &Tables) -> bool,
        relight: bool,
    ) {
        let bits = self.world.state_bits;
        let tables = self.tables.clone();
        let Some(col) = self.world.grid.get_mut(cx, cz) else { return };
        let Some(sec) = usize::try_from(sy).ok().and_then(|i| col.sections.get_mut(i)) else { return };
        // Block entities: removal, new state or creation (`block-entities.md` § 1.1).
        for &(i, state) in changes {
            if usize::from(i) < rc_world::SECTION_BLOCKS {
                let i32i = i32::from(i);
                let pos =
                    [cx * 16 + (i32i & 15), self.world.min_y + sy * 16 + (i32i >> 8), cz * 16 + ((i32i >> 4) & 15)];
                let old = sec.blocks.get(usize::from(i));
                self.block_entities.on_block_change(pos, old, state);
                let block = |s: u32| self.data.states.get(s as usize).map(|s| s.block);
                self.tech.on_block_change(pos, block(old) == block(state));
            }
        }
        // Changes that touch the light: local relight before the next meshing.
        for &(i, state) in changes.iter().filter(|&&(i, _)| relight && usize::from(i) < rc_world::SECTION_BLOCKS) {
            let old = sec.blocks.get(usize::from(i));
            if self.light.matters(old, state) {
                let i = i32::from(i);
                let p = [cx * 16 + (i & 15), sy * 16 + (i >> 8), cz * 16 + ((i >> 4) & 15)];
                self.relight.push(p);
                if self.light.matters_sky(old, state) {
                    self.relight_sky.push(p);
                }
            }
        }
        // Cells whose state really changes: only their sections and neighbours are remeshed.
        let changed: Vec<u16> = changes
            .iter()
            .filter(|&&(i, s)| usize::from(i) < rc_world::SECTION_BLOCKS && sec.blocks.get(usize::from(i)) != s)
            .map(|&(i, _)| i)
            .collect();
        sec.set_blocks(changes, bits, |s| air(s, &tables));
        // Client height maps, after the section (cells really changed only).
        let height = self.world.height as usize;
        for &i in &changed {
            let i = usize::from(i);
            let state = col.sections[sy as usize].blocks.get(i);
            col.update_heightmaps(i & 15, sy as usize * 16 + (i >> 8), (i >> 4) & 15, state, &self.world.hm_rules, height);
        }
        self.world.touch_blocks(cx, sy, cz, changed.into_iter());
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let b = weather::Blocking { data: &self.data, phys: &self.phys };
        self.weather.on_blocks(&view, [cx * 16, sy * 16 + self.world.min_y, cz * 16], changes, &b);
    }

    /// Starts the meshing of the modified columns (at most four tasks per pool thread,
    /// `RUSTCRAFT_MESHQUEUE`: a deep queue keeps the pool busy during the long frames of
    /// loading; bench of 19/10 in `docs/perf.md`).
    fn schedule_meshing(&mut self) {
        static DEPTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let depth =
            *DEPTH.get_or_init(|| rc_data::env_str!("RUSTCRAFT_MESHQUEUE").and_then(|v| v.parse().ok()).unwrap_or(4));
        let limit = rayon::current_num_threads() * depth;
        let free = limit.saturating_sub(self.in_flight);
        if free == 0 || self.world.dirty.is_empty() {
            return;
        }
        // The closest to the camera first: the world appears around the player.
        let (ex, ez) = ((self.eye[0] / 16.0).floor() as i32, (self.eye[2] / 16.0).floor() as i32);
        // A column already in progress waits for its result (base of the next remesh), unless it
        // must be redone whole.
        let meshes = &self.world.meshes;
        let mut ready: Vec<(i32, i32)> =
            self.world.dirty.iter().filter(|&(&k, &m)| meshes.may_start(k, m)).map(|(&k, _)| k).collect();
        let dist = |&k: &(i32, i32)| remesh::column_dist2(k, (ex, ez));
        if ready.len() > free {
            ready.select_nth_unstable_by_key(free - 1, dist);
            ready.truncate(free);
        }
        ready.sort_unstable_by_key(dist);
        for (x, z) in ready {
            let Some(mask) = self.world.dirty.remove(&(x, z)) else { continue };
            let Some(center) = self.world.grid.get_arc(x, z) else { continue };
            let version = self.world.versions.get(&(x, z)).copied().unwrap_or(0);
            let mut job = MeshJob::gather(center, version, |x, z| self.world.grid.get_arc(x, z));
            let (reuse, base_version) = match self.world.mesh_reuse((x, z), mask, version) {
                Some((s, (w, v))) => (Some((s, w)), Some(v)),
                None => (None, None),
            };
            job.reuse = reuse;
            let live = !mask.is_all();
            self.world.meshes.note_job(&job);
            job.shade = self.shade;
            job.has_sky = self.env.has_sky;
            job.opts = self.video.mesh;
            job.plains_biome = self.env.plains();
            (job.biome_zoom_seed, job.min_y) = (crate::environ::biome_zoom_seed(), self.world.min_y);
            let (tables, tx, min_y, palette) =
                (self.tables.clone(), self.meshes_tx.clone(), self.world.min_y, self.palette.clone());
            self.in_flight += 1;
            let n = job.center.sections.len();
            let secs = job.reuse.as_ref().map_or(n, |(m, _)| m.count(n));
            // Priority updates (`prioritizeChunkUpdates`): in-place meshing, visible in
            // this frame (never delayed by the upload budget).
            let urgent = self.video.mesh_sync(x, z, self.eye, self.options.i("prioritizeChunkUpdates"));
            let run = move || {
                let t = Instant::now();
                let m = blocks::mesh(&job, &tables, &palette, min_y);
                perf_log::mesh_work(t.elapsed(), secs);
                let t = Instant::now();
                let base = job.reuse.as_ref().zip(base_version).map(|((_, w), v)| (w, v));
                let done = remesh::finish(m, live, urgent, base);
                perf_log::prep_work(t.elapsed());
                let _ = tx.send(done);
            };
            if urgent {
                run();
                continue;
            }
            rayon::spawn(run);
        }
    }

    fn fly(&mut self, dt: f64) {
        let speed = if self.binds.down(keys::SPRINT) { 60.0 } else { 15.0 } * dt;
        let (s, c) = (f64::from(self.yaw.to_radians()).sin(), f64::from(self.yaw.to_radians()).cos());
        let (fwd, right) = ([-s, c], [-c, -s]);
        let mut d = [0.0f64; 3];
        let pressed = |i: usize| self.binds.down(i);
        if pressed(keys::FORWARD) {
            d[0] += fwd[0];
            d[2] += fwd[1];
        }
        if pressed(keys::BACK) {
            d[0] -= fwd[0];
            d[2] -= fwd[1];
        }
        if pressed(keys::RIGHT) {
            d[0] += right[0];
            d[2] += right[1];
        }
        if pressed(keys::LEFT) {
            d[0] -= right[0];
            d[2] -= right[1];
        }
        if pressed(keys::JUMP) {
            d[1] += 1.0;
        }
        if pressed(keys::SNEAK) {
            d[1] -= 1.0;
        }
        for (e, d) in self.eye.iter_mut().zip(d) {
            *e += d * speed;
        }
    }

    fn frame(&mut self) {
        // Only what arrived before the frame: a flood of chunks does not delay it.
        profiler::push("scheduledPacketProcessing");
        let (t_recv, n_recv) = (Instant::now(), self.session.events.len());
        for _ in 0..n_recv {
            let Ok(ev) = self.session.events.try_recv() else { break };
            self.deliver(ev);
        }
        perf_log::received(t_recv.elapsed(), n_recv);
        profiler::pop_push("scheduledExecutables");
        // Out of the world: title screen, servers, connection (`flow.rs`).
        if !self.front.in_world {
            self.menu_frame();
            return;
        }
        // Light recomputed after the frame's packets, before the meshing (like the game).
        if !self.relight.is_empty() {
            let changes = std::mem::take(&mut self.relight);
            let sky = std::mem::take(&mut self.relight_sky);
            let (grid, height, has_sky) = (&mut self.world.grid, self.world.height, self.env.has_sky);
            let touched = if light_engine::opts().sky_bit {
                light_engine::relight_masked(grid, height, &self.light, has_sky, &changes, &sky)
            } else {
                light_engine::relight_sections(grid, height, &self.light, has_sky, &changes)
            };
            for ((x, sy, z), edges) in touched {
                if let Some(s) = &mut self.light_shadow {
                    s.touch(x, z);
                }
                self.world.touch_around(x, sy, z, edges, true);
            }
            self.weather.rev += 1;
        }
        self.light_recv.clear();
        if let Some(s) = &mut self.light_shadow
            && self.solo.is_some()
        {
            for ((x, z), sky, block) in s.check(&self.world.grid) {
                eprintln!("singleplayer light ({x}, {z}): differences with the server sky {sky}, block {block}");
            }
        }
        self.schedule_meshing();
        profiler::pop_push("mouse");
        self.mouse_frame();
        profiler::pop();
        let now = Instant::now();
        self.debug.on_frame(now, &self.session.link.stats);
        let elapsed = now.duration_since(self.last).as_secs_f64();
        let dt = elapsed.min(0.1);
        self.last = now;
        if self.free_cam {
            self.fly(dt);
        }
        // Singleplayer pause from the top screen of the previous frame (`pause.rs`).
        self.update_pause();
        // All the elapsed time counts: a hitch is caught up (at most 10 ticks per frame).
        let due = ticking::due_ticks(&mut self.tick_acc, elapsed, self.exact.rate.tick_len());
        if due > 0 {
            self.animate_textures();
        }
        profiler::push("tick");
        let depth = profiler::depth();
        for _ in 0..due {
            self.game_tick();
            profiler::unwind(depth);
        }
        profiler::pop();
        if !self.free_cam && self.positioned {
            // Player's eyes, interpolated between the last two ticks.
            let alpha = self.partial();
            let eye = self.player.eye_height();
            for a in 0..3 {
                self.eye[a] = self.prev_pos[a] + (self.player.pos[a] - self.prev_pos[a]) * alpha;
            }
            self.eye[1] += eye;
        }
        let shot = self.place_camera(self.partial() as f32);
        self.shot = shot;
        let aspect = self.renderer.as_ref().map_or(16.0 / 9.0, |r| r.aspect());
        self.env.eye_effects = self.video.darkness.fog(self.partial() as f32);
        self.env.effects = environ::CameraEffects::read(&self.hud_extra.effects, &self.hud_extra.effect_names);
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let frame_env = self.env.frame(
            &view,
            &self.phys,
            &self.data,
            shot.pos,
            shot.yaw,
            shot.pitch,
            aspect,
            self.partial() as f32,
            dt,
            self.options.i("renderDistance"),
            self.player.eyes_in_water,
        );
        // Crosshair aiming and outline, before borrowing the renderer.
        // Listener: eyes of the camera and its orientation (`sounds.md` § 3.4).
        profiler::push("sound");
        // Without joining: kept while the engine is being built.
        let (_, up, fwd) = camera::axes(shot.yaw, shot.pitch);
        self.audio.set_listener(shot.pos, fwd.to_array(), up.to_array());
        profiler::pop_push("frame");
        profiler::push("update");
        let hit = self.pick();
        let cracks = self.crack_vertices();
        self.hud.attack = Some(self.attack_indicator(hit));
        self.hud.attack_mode = self.options.i("attackIndicator");
        self.hud.main_left = self.options.s("mainHand") == "left";
        self.hud_frame(hit);
        let outline = self.outline_target(hit);
        let screen_mouse = self.gui_mouse();
        // F3 screen: lines collected as soon as an entry is active (text or not).
        let debug_text = self.debug_collect(hit);
        self.hud.crosshair_3d = self.debug.is_on("3d_crosshair") && !self.camera.view.detached();
        // Resource reload ready: applied before drawing (`reload.rs`).
        self.reload_tick();
        let (m5_hide_hand, assets) = (self.m5_hide_hand(), self.assets.clone());
        // Top screen (deferred subtitles or not, `pause.rs`).
        let (top, partial) = (self.pause_top(), self.partial() as f32);
        // Hands of the local player for the frame's item models (cast rod, selected slot).
        self.publish_local_hands();
        // Control of the remeshing (`remesh.rs`) at the time of the final capture.
        if remesh::checking()
            && self.capture.as_ref().is_some_and(|(_, at)| self.started.elapsed().as_secs_f64() >= *at)
        {
            self.mesh_check_report();
        }
        let Some(r) = &mut self.renderer else { return };
        let t_up = Instant::now();
        let o = remesh::opts();
        let mut batch = std::mem::take(&mut self.mesh_pending);
        batch.extend(self.meshes_rx.try_iter());
        if let Some(budget) = o.budget {
            let eye = ((self.eye[0] / 16.0).floor() as i32, (self.eye[2] / 16.0).floor() as i32);
            self.mesh_pending = remesh::split_budget(&mut batch, eye, budget);
        }
        let (mut t_done, mut t_gpu, mut sent) = (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        for d in batch {
            self.in_flight -= 1;
            perf_log::meshed(d.mesh.x, d.mesh.z);
            remesh::note_received();
            let t = Instant::now();
            let done = self.world.mesh_done(d);
            let t_send = Instant::now();
            t_done += t_send - t;
            if let Some(remesh::Done { mesh, prep, in_place, .. }) = done {
                self.misc.column_meshed(mesh.x, mesh.z);
                r.upload(&mesh, prep, in_place);
                // Large arrays freed on the pool, not on the frame thread.
                if o.pack == remesh::PackAt::Pool {
                    rayon::spawn(move || drop(mesh));
                }
                sent += 1;
                t_gpu += t_send.elapsed();
            }
        }
        perf_log::uploaded(t_up.elapsed(), t_done, t_gpu, sent);
        remesh::reclaim_pool();
        if capseq::frame(r, self.started) {
            self.quit_after_capture = true;
        }
        if let Some((path, at)) = &self.capture
            && self.started.elapsed().as_secs_f64() >= *at
        {
            r.request_capture(path.clone());
            self.capture = None;
            self.quit_after_capture = true;
        } else if self.quit_after_capture {
            // Light engine check: RUSTCRAFT_LIGHTDUMP=<file> writes the light of the
            // columns around the player (radius 2), to compare two runs.
            if let Some(path) = rc_data::env_os!("RUSTCRAFT_LIGHTDUMP") {
                let (cx, cz) = ((self.player.pos[0].floor() as i32) >> 4, (self.player.pos[2].floor() as i32) >> 4);
                let mut out = Vec::new();
                for x in cx - 2..=cx + 2 {
                    for z in cz - 2..=cz + 2 {
                        let Some(col) = self.world.grid.get(x, z) else { continue };
                        for layers in [&col.block_light, &col.sky_light] {
                            for l in layers.iter() {
                                match l {
                                    Some(l) => out.extend_from_slice(&l[..]),
                                    None => out.extend_from_slice(&[0xff; rc_world::LIGHT_BYTES]),
                                }
                            }
                        }
                    }
                }
                let _ = std::fs::write(path, out);
            }
            rc_assets::cache::wait_writer();
            self.close_solo();
            std::process::exit(0);
        }
        let (ww, wh) = r.size();
        profiler::pop_push("extract");
        r.gui.begin(ww, wh);
        if rc_data::env_flag!("RUSTCRAFT_GUITEST") {
            let q = r.queue_handle();
            let g = &mut r.gui;
            g.sprite("hud/hotbar", g.width / 2.0 - 91.0, g.height - 22.0, 182.0, 22.0, 0xffff_ffff);
            let line = gui::plain("Rustcraft §cText §lbold §r§oitalic§r §nunderlined§r é 中 한");
            g.text(&q, &line, 2.0, 2.0, 0xffff_ffff, true, 0);
            let tr = rc_text::flatten(
                &rc_text::Component::translate(
                    "chat.type.text",
                    vec![rc_text::Arg::Text("Bench".into()), rc_text::Arg::Text("hello".into())],
                ),
                &g.lang,
            );
            g.text(&q, &rc_text::styled_chars(&tr), 2.0, 12.0, 0xffff_ffff, true, 0x8000_0000);
        }
        // Items of the local player (bar, menus, first-person hand). Fields taken one by one
        // (the renderer is borrowed): like `hud_env`.
        let env = stack::HudEnv {
            dimension: self.dimension.clone(),
            tick: (self.started.elapsed().as_secs_f64() * 20.0) as i64,
            game_time: self.env.env.clocks.game_time,
            sun_angle: self.sun_angle,
            pos: self.player.pos,
            // Body rotation not yet tracked: that of the head.
            body_yaw: self.yaw,
            spawn: self.spawn.clone(),
            last_death: self.last_death.clone(),
            keys: self.binds.down_mask(),
            camera_elsewhere: self.exact.camera.is_some(),
            ticking: self.exact.rate.runs,
            team: self.scores.team_rgb(&self.local_name),
        };
        {
            let q = r.queue_handle();
            let max_health = self.player.attribute("minecraft:max_health").unwrap_or(20.0);
            let armor = self.player.attribute("minecraft:armor").unwrap_or(0.0);
            let items = hud::HotbarItems {
                inv: &self.inventory,
                ids: &self.component_ids,
                data: &self.data,
                infos: &self.data.items,
                names: &self.item_names,
                env: &env,
                pt: partial,
            };
            // Location bar: markers seen from the camera (eyes of the known tracked entities).
            let pt = partial;
            let dims = r.entities.as_ref().map_or(&[][..], |e| e.dims.as_slice());
            let entity = |uuid: u128| {
                let e = self.entities.map.values().find(|e| e.uuid == uuid)?;
                let feet = e.render_pos(pt);
                Some((feet, [feet[0], feet[1] + f64::from(e.dims(dims)[1]), feet[2]]))
            };
            self.hud.more.locate(&hud_more::LocateIn {
                cam: shot.pos,
                yaw: shot.yaw,
                pitch: shot.pitch,
                fov: r.view.fov,
                me: self.player.pos,
                me_uuid: self.local_uuid,
                entity: &entity,
            });
            self.hud.draw(&mut r.gui, &q, self.env.game_mode, max_health, armor, Some(&items));
            for s in self.hud.more.take_sounds() {
                if let Some(a) = self.audio.get() {
                    let (cat, pos) = if s.ambient {
                        (rc_audio::Category::Ambient, None)
                    } else {
                        (rc_audio::Category::Player, Some(self.player.pos))
                    };
                    a.play(s.name, cat, pos, s.volume, s.pitch, None);
                }
            }
            // Spectator menu in place of the item bar (`spectator.rs`).
            if self.env.game_mode == 3 && !self.hud.hidden {
                let backdrop = if self.options.b("backgroundForChatOnly") {
                    0
                } else {
                    ((self.options.f("textBackgroundOpacity") * 255.0) as u32) << 24
                };
                let options = &self.options;
                self.spectator.draw(&mut r.gui, &q, backdrop, &|u| spectator::default_face(&assets, u), &|s| {
                    options.s(spectator::HOTBAR_KEY_OPTIONS[s])
                });
            }
            // HUD effects hidden when the inventory screen shows them (`inv_effects.rs`).
            if !self.hud.hidden {
                if inv_effects::screen_column(&r.gui, self.creative_ui.open, &self.screens, &self.inventory).is_none() {
                    self.hud_extra.draw_effects(&mut r.gui);
                }
                self.hud_extra.draw_bosses(&mut r.gui, &q);
            }
            // Sleep fade (even with the HUD hidden), under the sidebar and the titles.
            self.hud.more.draw_sleep(&mut r.gui);
            if !self.hud.hidden {
                self.scores.draw_sidebar(&mut r.gui, &q, &self.local_name, self.hud.text_background);
                // Action bar then title (the title covers it), text background outside the chat.
                let bg = hud::background_color(self.hud.text_background, 0);
                self.chat.draw_action_bar(&mut r.gui, &q, pt, bg);
                self.hud_extra.draw_title(&mut r.gui, &q, pt, bg);
            }
            // Menus over the chat (restrictions screen): chat drawn closed. Closed chat
            // hidden with the HUD (F1); open, it is a screen and stays drawn.
            self.chat.covered = self.menus.is_open();
            if pause::chat_drawn(self.hud.hidden, self.chat.is_open()) {
                self.chat.draw(&mut r.gui, &q, partial, screen_mouse);
            }
            if self.chat.is_open() && !self.chat.covered {
                self.cmd.draw(&mut r.gui, &q, &mut self.chat, screen_mouse);
            }
            // Chat in bed: "Leave Bed" button (`bed.rs`).
            self.bed.draw(&mut r.gui, &q, screen_mouse, self.chat.is_open() && !self.chat.covered);
            // Player list while Tab is held (game keys inactive during input).
            let tab = self.binds.down(keys::PLAYERLIST) || rc_data::env_flag!("RUSTCRAFT_TABTEST");
            let tab = tab && tablist::visible(self.solo.is_some(), &self.players, &self.scores);
            self.tab_list.set_visible(tab);
            if tab && !self.chat.is_open() && !self.hud.hidden {
                r.gui.next_layer();
                let ents = &self.entities;
                let ctx = tablist::Ctx {
                    online: self.online_mode,
                    tick: i64::from(self.hud.gui_ticks()),
                    background: hud::background_color(self.hud.text_background, 0x20ff_ffff),
                    in_world: &|u| {
                        ents.me.as_ref().is_some_and(|e| e.uuid == u)
                            || ents.map.values().any(|e| e.uuid == u && e.kind == "player")
                    },
                    default_skin: &|u| tablist::default_skin(&assets, u),
                };
                self.tab_list.draw(&mut r.gui, &q, &self.players, &ctx, &self.scores);
            }
            // Subtitles (last element of the HUD) and system notification (`misc.rs`).
            let subs = self.options.b("showSubtitles");
            if let Some(a) = self.audio.get() {
                a.set_subtitles(subs);
                let now = std::time::Instant::now();
                for s in a.take_subtitles() {
                    self.misc.subtitles.notify(s, now);
                }
            }
            // Lines never erased (rank of first appearance kept for the whole session). Without a
            // screen, drawn last (above F3); under an in-game screen, on its veil.
            let subs_at = if subs { pause::subtitles_at(self.hud.hidden, top) } else { pause::Subs::Off };
            let opacity =
                if self.options.b("backgroundForChatOnly") { 0.8 } else { self.options.f("textBackgroundOpacity") };
            let subs_style = subtitles::Style {
                duration: self.options.f("notificationDisplayTime"),
                background: ((opacity * 255.0) as u32) << 24,
            };
            if subs_at == pause::Subs::Before {
                self.misc.subtitles.draw(&mut r.gui, &q, shot.pos, shot.yaw, shot.pitch, &subs_style);
            }
            self.misc.draw_toast(&mut r.gui, &q, self.options.f("notificationDisplayTime"));
            if self.screens.open {
                let me = self.entities.local;
                self.screens.player_model = self.entities.me.is_some().then_some(me);
                self.screens.draw(&mut r.gui, &q, &self.inventory, &items, screen_mouse);
                let ctx = recipe_book::Ctx { data: &self.data, rec: &self.creative_rec, item_names: &self.item_names };
                let ench = &self.screens.hasher.enchantments;
                let link = &self.session.link;
                self.recipe_book.draw(&mut r.gui, &q, &self.inventory, &items, ench, &ctx, screen_mouse, link);
            }
            if self.creative_ui.open {
                let me = self.entities.local;
                self.creative_ui.player_model = self.entities.me.is_some().then_some(me);
                let t = self.started.elapsed().as_secs_f32();
                let ench = &self.screens.hasher.enchantments;
                self.creative_ui.draw(&mut r.gui, &q, &self.inventory, &items, screen_mouse, ench, t);
            }
            if let Some((col, top)) =
                inv_effects::screen_column(&r.gui, self.creative_ui.open, &self.screens, &self.inventory)
            {
                let (fx, names, rate) = (&self.hud_extra.effects, &self.hud_extra.effect_names, self.exact.rate.rate);
                inv_effects::draw(&mut r.gui, &q, fx, names, rate, col, top, screen_mouse);
            }
            self.special.draw(&mut r.gui, &q, &items, screen_mouse, self.started.elapsed().as_secs_f64());
            op_screens::draw_suggestions(&mut self.special, &mut self.cmd, &mut r.gui, &q, screen_mouse);
            if let Some(d) = &mut self.death {
                let s = f64::from(r.gui.scale);
                let mouse = self.cursor.map(|(x, y)| ((x / s) as f32, (y / s) as f32));
                d.draw(&mut r.gui, &q, mouse);
            }
            // Server dialog (`dialog.rs`).
            if let Some(d) = &mut self.exact.dialog {
                let s = f64::from(r.gui.scale);
                let mouse = self.cursor.map(|(x, y)| ((x / s) as f32, (y / s) as f32));
                d.draw(&mut r.gui, &q, mouse, Some(&items), &self.screens.hasher.enchantments);
            }
            if self.menus.is_open() && self.packs.screen.is_none() && self.progress.screen.is_none() {
                self.menus.draw(&mut r.gui, &q, &self.options, screen_mouse, self.started.elapsed().as_secs_f64());
                let audible = self.options.f("soundCategory_master") * self.options.f("soundCategory_music") > 0.0;
                if self.menus.top() == Some(menus::Id::Pause) && self.options.s("musicToast") != "never" && audible {
                    self.fx.music.draw_pause(&mut r.gui, &q, self.started.elapsed().as_secs_f64() * 1000.0);
                }
            }
            // Advancements and statistics screens (`progress_app.rs`).
            let ctx = progress_app::DrawCtx {
                assets: &self.assets,
                blur: self.options.i("menuBackgroundBlurriness"),
                time: self.started.elapsed().as_secs_f64(),
            };
            self.progress.draw(&mut r.gui, &q, &items, &ctx, screen_mouse);
            // Autosave indicator (singleplayer, `world_list.rs`): above the open screen,
            // even with the HUD hidden, smoothed by the frame duration (in ticks), like the game.
            let bd = if self.options.b("backgroundForChatOnly") {
                0
            } else {
                ((self.options.f("textBackgroundOpacity") * 255.0) as u32) << 24
            };
            let real = ticking::realtime_delta(elapsed);
            world_list::draw_autosave(&mut r.gui, &q, real, self.options.b("showAutosaveIndicator"), bd);
            // Notifications above the screens (manager drawn after the open screen); updated
            // even with the HUD hidden, only the drawing is skipped.
            let (now, mult) =
                (self.started.elapsed().as_secs_f64() * 1000.0, self.options.f("notificationDisplayTime"));
            self.toasts.periodic_tick(&r.gui, now);
            self.toasts.update(now, mult);
            self.fx.music.update(now);
            for s in self.toasts.sounds.drain(..) {
                if let Some(a) = self.audio.get() {
                    a.play(s, rc_audio::Category::Ui, None, 1.0, 1.0, None);
                }
            }
            if !self.hud.hidden {
                self.toasts.draw(&mut r.gui, &q, &items, now, mult);
                if self.options.s("musicToast") == "pause_and_toast" && self.menus.top() != Some(menus::Id::Pause) {
                    self.fx.music.draw_toast(&mut r.gui, &q);
                }
            }
            // Hover tooltips and link confirmation screen (`links.rs`).
            let (book, death) = (links::book_of(&mut self.special.overlay), self.death.as_ref());
            let advanced = self.options.b("advancedItemTooltips");
            links::draw_overlay(&mut r.gui, &q, &mut self.chat, book, death, screen_mouse, &self.data, advanced);
            if !self.menus.is_open()
                && let Some(m) = self.chat.test_mouse.or(screen_mouse)
                && links::clickable_under(
                    &r.gui,
                    &mut self.chat,
                    links::book_of(&mut self.special.overlay),
                    self.death.as_ref(),
                    m,
                    self.mods.shift_key(),
                )
            {
                widgets::request_cursor(widgets::Cursor::Hand);
            }
            // End screen (`win_screen.rs`): alone, on the End portal background.
            if let Some(w) = &mut self.win {
                let (ww, wh) = r.size();
                r.gui.begin(ww, wh);
                w.draw(&mut r.gui, &q, self.exact.rate.tick_len());
            }
            // World loading screen (`flow.rs`): alone, on the panorama.
            if self.front.is_open() {
                let (ww, wh) = r.size();
                r.gui.begin(ww, wh);
                self.front.draw(&mut r.gui, &q, &self.options, screen_mouse, self.started.elapsed().as_secs_f64());
            }
            // Server pack prompt, resource loading screen (`reload.rs`).
            self.packs.draw(&mut r.gui, &q, &self.options, screen_mouse);
            if debug_text {
                self.debug.draw(&mut r.gui, &q, (ww, wh), self.options.i("debugGuiScale"), self.options.i("maxFps"));
            }
            // Under an in-game screen, interleaved between its veil and its panel.
            let st = &mut self.misc.subtitles;
            match subs_at {
                pause::Subs::Deferred => st.draw(&mut r.gui, &q, shot.pos, shot.yaw, shot.pitch, &subs_style),
                pause::Subs::InScreen => {
                    r.gui.draw_after_veil(|g| st.draw(g, &q, shot.pos, shot.yaw, shot.pitch, &subs_style));
                }
                pause::Subs::Off | pause::Subs::Before => {}
            }
            // Game mode switcher (F3+F4).
            if let Some(s) = &mut self.debug.switcher {
                s.prepare(&debug_switcher::Icons {
                    models: r.gui.items.as_ref().map(|a| &a.models),
                    names: &self.item_names,
                    infos: &self.data.items,
                    ids: &self.component_ids,
                    data: &self.data,
                    env: &env,
                });
                let f4 = options::key_label(&r.gui.lang, self.options.s("key_key.debug.switchGameMode"));
                s.draw(&mut r.gui, &q, screen_mouse, &f4);
            }
            let (scale, time) = (self.options.i("debugGuiScale"), self.started.elapsed().as_secs_f64());
            self.debug.menu_blur = self.options.i("menuBackgroundBlurriness");
            self.debug.draw_options(&mut r.gui, &q, screen_mouse, scale, time);
            // IME composition of the focused field: over the interface, under the tooltips (`ime.rs`).
            ime::finish_frame(&mut r.gui, &q);
        }
        let pt = partial;
        self.env.darkness = self.video.darkness.light(pt, self.options.f("darknessEffectScale") as f32);
        if let Some(lm) = self.env.lightmap(pt) {
            r.set_lightmap(&lm);
            self.debug.logs.lightmap = Some(lm); // F3+4 (`debug_charts.rs`)
        }
        // `RUSTCRAFT_NOENT`: entities not drawn (measuring their cost).
        // Projection of the frame: field of view, damage shake, sway, hand.
        {
            let pt = partial;
            let hurt =
                self.entities.me.as_ref().filter(|e| e.hurt_time > 0).map(|e| (e.hurt_time as f32 - pt, e.hurt_dir));
            let mut view = self.camera.render_view(pt, frame_env.fluid, hurt);
            view.hand &= !self.free_cam && !self.hud.hidden && self.env.game_mode != 3 && !m5_hide_hand;
            // Hand pass also for the totem animation (even in third person).
            self.ev.totem.show_hand = view.hand;
            view.hand |= self.ev.totem.active() && !self.hud.hidden;
            r.view = view;
            // Screen effects, camera overlays, portal and nausea distortion.
            let wv = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let state_at = |x, y, z| wv.state(x, y, z);
            let mut eye = self.player.pos;
            eye[1] += self.player.eye_height();
            let (fx, distort) = self.hud.more.scene(&hud_more::SceneIn {
                data: &self.data,
                state_at: &state_at,
                me: self.entities.me.as_ref(),
                equippable: self.component_ids.equippable,
                feet: self.player.pos,
                eye,
                yaw: self.yaw,
                pitch: self.pitch,
                shot: (shot.yaw, shot.pitch),
                fov: r.view.fov,
                aspect,
                hud_fov: r.view.hud_fov,
                first_person: !self.camera.view.detached() && !self.free_cam,
                spectator: self.env.game_mode == 3,
                hud_hidden: self.hud.hidden,
                screen_effect_scale: self.options.f("screenEffectScale") as f32,
                eye_brightness: self.env.eye_brightness(&wv, eye),
                pt,
                tick_len: self.exact.rate.tick_len() as f32,
            });
            r.distort = distort;
            r.prepare_overlays(&fx);
        }
        let in_view = r.visibility(shot.pos, shot.yaw, shot.pitch, frame_env.far);
        r.leash.verts.clear();
        // Screen effects (§ 6.2): server list then observed entity, only with a player.
        let cam_kind = self.exact.camera.and_then(|id| self.entities.map.get(&id)).map(|e| e.kind.as_str());
        r.post_effects = self.misc.effects(self.exact.camera, cam_kind, self.camera.view.detached());
        r.post_blur = self.options.i("menuBackgroundBlurriness");
        let win_size = r.size();
        if let Some(er) = r.entities.as_mut().filter(|_| !rc_data::env_flag!("RUSTCRAFT_NOENT")) {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let origin = [shot.pos[0].floor() as i32, shot.pos[2].floor() as i32];
            let pt = partial;
            let (darken, has_sky, ambient) = self.env.shadow_params();
            if rc_data::env_flag!("RUSTCRAFT_ENTCHECK") && self.fps_frames == 10 {
                let names = self.data.registry("minecraft:entity_type").map(|r| r.entries.clone()).unwrap_or_default();
                eprintln!("kinds without render: {}", er.unresolved(&names).join(" "));
            }
            for (name, entries) in self.pending_registries.drain(..) {
                er.set_variants(&name, &entries);
            }
            er.set_camera(shot.yaw, shot.pitch);
            // Skins of the placed player heads.
            let mut patched = Vec::new();
            for (key, value) in self
                .block_entities
                .heads
                .drain(..)
                .chain(er.pending_heads.drain(..))
                .chain(item_special::take_wanted())
                .chain(r.gui.objects.wanted.drain(..))
            {
                // Texture imposed by the profile's skin patch: game resource, read here.
                if value.starts_with("asset:") {
                    patched.push((key, value));
                } else {
                    self.skins.request_head(key, value);
                }
            }
            for (key, value) in patched {
                let id = &value["asset:".len()..];
                let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
                if let Some(img) = self.assets.image(&format!("assets/{ns}/textures/{path}.png")) {
                    if let Some(items) = r.gui.items.as_mut() {
                        items.set_head_skin(&r.queue, key, &img);
                    }
                    er.set_skin(key, Some((img, false, value)), None, None);
                }
            }
            for (key, skin) in item_special::take_default_heads() {
                if let (Some(items), Some(img)) = (r.gui.items.as_mut(), item_special::default_skin_image(&self.assets, skin)) {
                    items.set_head_skin(&r.queue, key, &img);
                }
            }
            for (uuid, s) in self.skins.poll() {
                // Without skin (cape or elytra alone): the icons keep the default skin.
                if let Some((image, _, _)) = &s.skin {
                    // Interface icons of the player heads: skin of the head's identity.
                    if let Some(items) = r.gui.items.as_mut() {
                        items.set_head_skin(&r.queue, uuid, image);
                    }
                    self.spectator.skin(uuid, image);
                    self.tab_list.skin(uuid, image);
                    r.gui.objects.skin(uuid, image);
                }
                er.set_skin(uuid, s.skin, s.cape, s.elytra);
            }
            let items = r.gui.items.as_ref().map(|a| &a.models);
            // Local player only in third person (no name above oneself).
            // Camera on another entity: the local player is not drawn, nor the followed
            // entity in first person (`set_camera`).
            let shown = draws_local_player(
                self.camera.view.detached(),
                // Lying: bed position (value 14), like `sleeping_pos` (render borrowed here).
                self.entities.me.as_ref().is_some_and(|m| {
                    matches!(m.meta.get(&14), Some(rc_proto_v777::entity::Meta::BlockPos(Some(_))))
                }),
                self.free_cam,
                self.exact.camera.is_some(),
            );
            let me = self.entities.me.as_ref().filter(|_| shown);
            let hidden = self.exact.camera.filter(|_| !self.camera.view.detached());
            // Flying copies of the picked-up items, drawn like their entity (`misc.rs`).
            let pickups = self.misc.pickup_copies(pt);
            er.glow = glow_colors(&self.entities, &self.players, &self.scores);
            er.ghosts = ghost_set(&self.entities, &self.players, &self.scores, self.env.game_mode == 3);
            er.spectators = spectator_set(&self.entities, &self.players, self.env.game_mode == 3);
            er.world.clone_from(&env);
            er.camera = self.exact.camera;
            er.upside_down = (self.players.map.iter())
                .filter(|(_, p)| p.name == "Dinnerbone" || p.name == "Grumm")
                .map(|(&u, _)| u)
                .collect();
            er.extra_ears = self.players.map.iter().filter(|(_, p)| p.name == "deadmau5").map(|(&u, _)| u).collect();
            // Bobbers and fishing lines, text displays, guardian beams (`objects.rs`).
            let obj_frame = entity_render::ObjectsFrame {
                pt,
                origin,
                cam: shot.pos,
                local: self.player_id,
                local_left: self.options.s("mainHand") == "left",
                first_person: !self.camera.view.detached(),
                fov: self.options.fov(),
                yaw: shot.yaw,
                pitch: shot.pitch,
                aspect: win_size.0 as f32 / win_size.1.max(1) as f32,
            };
            er.objects_prepass(&self.entities, &obj_frame);
            // Biome colours of a block (falling blocks, pushed by a piston, transient).
            let (palette, plains) = (&self.palette, self.env.plains());
            let biome_tints = |x, y, z| view.biome_tints(palette, x, y, z, plains);
            er.prepare(
                self.entities
                    .map
                    .iter()
                    .filter(|(id, _)| Some(**id) != hidden)
                    .map(|(_, e)| e)
                    .chain(me)
                    .chain(&pickups),
                pt,
                origin,
                shot.pos,
                &|x, y, z| view.light_pair(x, y, z),
                &in_view,
                items,
                &entity_render::ShadowWorld {
                    state: &|x, y, z| view.state(x, y, z),
                    brightness: &|x, y, z| view.raw_brightness(x, y, z, has_sky, darken),
                    ambient,
                    biome_tints: &biome_tints,
                },
            );
            er.push_guardian_beams(&self.entities, &obj_frame);
            er.push_block_entities(
                &self.block_entities,
                &entity_render::BlockFrame {
                    pt,
                    origin,
                    cam: shot.pos,
                    game_time: self.env.env.clocks.game_time,
                    min_y: self.world.min_y,
                    light: &|x, y, z| view.light_pair(x, y, z),
                    visible: &in_view,
                    items,
                    render_distance: self.options.i("renderDistance").min(self.env.server_view.max(2)),
                    top: self.world.min_y + self.world.height,
                    state: &|x, y, z| view.state(x, y, z),
                    scoping: self.hud.more.scoping,
                    biome_tints: &biome_tints,
                },
            );
            // Transient blocks and leash ropes (`misc.rs`).
            let light = |x, y, z| view.light_pair(x, y, z);
            er.push_transient(self.misc.transient.iter().map(|t| (t.pos, t.state)), origin, &light, &biome_tints);
            let ctx = leash::Ctx {
                pt,
                table: &er.dims,
                local: self.player_id,
                local_left: self.options.s("mainHand") == "left",
                first_person: !self.camera.view.detached(),
            };
            r.leash.verts =
                misc::leash_vertices(&self.misc, &self.entities, &ctx, origin, shot.pos, &light, &in_view);
            // First-person hand: stack = inverse view × shake and sway × tilt.
            if r.view.hand
                && self.ev.totem.show_hand
                && let (Some(me), Some(models), Some(hs)) = (&self.entities.me, items, &self.camera.hand)
            {
                let base = camera::view_matrix(shot.pos, shot.yaw, shot.pitch).2.inverse()
                    * r.view.tilt
                    * self.camera.hand_sway(pt, shot.yaw, shot.pitch);
                let p = me.render_pos(pt);
                let eye_y = p[1] + self.camera.eye_height(pt);
                let (b, s) = view.light_pair(p[0].floor() as i32, eye_y.floor() as i32, p[2].floor() as i32);
                let light = (u32::from(b) * 16) | ((u32::from(s) * 16) << 8);
                let extra = hand::HandExtra {
                    enchantments: &self.screens.hasher.enchantments,
                    spin: self.player.spin_attack,
                    kinetic_ticks: self.ev.kinetic_since(self.player_id, self.env.env.clocks.game_time, pt),
                    env: &env,
                };
                hand::push(er, models, hs, me, pt, base, light, &self.component_ids, &self.data, &extra);
            }
            // Totem activated: full-screen item in front of the camera, drawn after the screen effects,
            // in the view frame with the items' fixed lights (`totem_anim.rs`).
            if r.view.hand
                && !self.hud.hidden
                && let (Some(models), Some((stack, m))) = (items, self.ev.totem.current(pt, aspect))
            {
                er.begin_totem();
                er.push_stack(models, stack, Some(totem_anim::FIXED), None, m, totem_anim::FULL_LIGHT);
            }
            // 3D interface models requested when drawing the screens (`special.rs`).
            special::push_models(er, &mut self.entities, items, pt);
        }
        // Text of the visible signs (faces collected by the block-entity render).
        if let Some(er) = &r.entities {
            let q = r.queue_handle();
            r.block_text.prepare(&mut r.gui, &q, &er.blocks.texts, &self.block_entities);
            let bg = self.hud.text_background.unwrap_or(63 << 24);
            r.text_displays.prepare(&mut r.gui, &q, er.text_draws(), er.text_labels(), &self.entities, bg);
        }
        // End screen: End portal background in front of the camera (`win_screen.rs`).
        if let Some(er) = r.entities.as_mut()
            && self.win.as_ref().is_some_and(|w| w.portal_background())
        {
            win_screen::push_backdrop(&mut er.blocks.fx, &shot);
        }
        let pt = partial;
        let pw = particles::TickWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            player: None,
            scope: None,
        };
        self.particles.prepare(&mut r.particles, &pw, shot.pos, shot.yaw, shot.pitch, pt, &in_view);
        {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let (rain, t) = (self.env.env.weather.rain, self.env.env.clocks.game_time);
            let radius = self.options.i("weatherRadius");
            self.weather.prepare(&mut r.weather, &view, &self.data, shot.pos, rain, t, pt, radius);
            let opt = self.options.i("renderDistance");
            let chunks = if self.env.server_view > 0 { opt.min(self.env.server_view) } else { opt };
            self.weather.prepare_wall(&mut r.weather.wall, shot.pos, pt, f64::from(chunks * 16), frame_env.far);
        }
        r.outline.set(outline.as_ref().map(|(shape, boxes, corner, tr)| (*shape, boxes.as_slice(), *corner, *tr)));
        if !r.cracks.loaded() {
            let images: Vec<_> = (0..20)
                .map(|i| {
                    let name = if i < 10 { format!("destroy_stage_{i}") } else { format!("destroy_oit_stage_{}", i - 10) };
                    rc_assets::texture::load(&self.assets, &format!("minecraft:block/{name}"))
                        .unwrap_or_else(|| rc_assets::texture::Image::missing(16))
                })
                .collect();
            r.cracks.load(&r.queue_handle(), &images);
        }
        r.cracks.set(cracks.0, cracks.1);
        // F3 traces (boxes, chunk borders, 3D crosshair): nothing if no entry is active.
        let scene = debug_render::Scene {
            debug: &self.debug,
            entities: &self.entities,
            eye: shot.pos,
            yaw: shot.yaw,
            pitch: shot.pitch,
            pt,
            camera: self.exact.camera.filter(|id| self.entities.map.contains_key(id)),
            first_person: !self.camera.view.detached(),
            free_cam: self.free_cam,
            player: &self.player,
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            border: &self.weather.border,
            data: &self.data,
            ids: &self.component_ids,
            crosshair: !self.camera.view.detached() && !self.hud.hidden && !self.free_cam,
            gui_scale: r.gui.scale,
        };
        debug_render::fill(r, &scene, &in_view);
        let win_w = r.size().0;
        if let Some(er) = &r.entities {
            er.push_fishing_lines(&mut r.debug_lines, win_w);
        }
        {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let visible = self.env.game_mode == 3 || (self.env.game_mode == 1 && self.debug.op_level >= 2);
            let state = |x, y, z| view.state(x, y, z);
            let rd = self.options.i("renderDistance").min(self.env.server_view.max(2));
            tech_blocks::draw_boxes(&mut r.debug_lines, &self.tech, shot.pos, visible, rd, &state, &self.data);
            let q = r.queue_handle();
            let now = std::time::Instant::now();
            tech_blocks::draw_markers(&mut r.debug_lines, &mut r.gui, &q, &mut self.tech, shot.pos, rd, now);
        }
        let target = match hit {
            pick::Hit::Entity { id, .. } => Some(id),
            _ => None,
        };
        // Background of the labels: 0.25, or the text background opacity if the option applies it everywhere.
        r.tags.set_background(if self.options.b("backgroundForChatOnly") {
            nametag::DEFAULT_OPACITY
        } else {
            self.options.f("textBackgroundOpacity") as f32
        });
        name_tags(
            r,
            &self.entities,
            &self.players,
            &self.world,
            shot.pos,
            (shot.yaw, shot.pitch),
            pt,
            self.hud.hidden,
            target,
            (&self.scores, &self.local_name, self.env.game_mode == 3),
            &self.component_ids,
        );
        r.show_panorama = self.front.is_open();
        if r.show_panorama && r.panorama.is_none() {
            if let Some(p) = panorama::PanoramaAssets::load(&self.assets) {
                r.enable_panorama(&p);
            }
        } else if !r.show_panorama && r.panorama.is_some() {
            // In a game the panorama is not used: its VRAM (24 MB) is given back.
            r.panorama = None;
        }
        profiler::pop_push("render");
        let drawn = r.render(shot.pos, shot.yaw, shot.pitch, self.world.min_y, self.world.height, &frame_env);
        profiler::pop_push("fpsUpdate");
        if drawn {
            self.fps_frames += 1;
        } else {
            // Surface without a frame (hidden window): no tight loop.
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        first_frame(self.started);
        if drawn {
            FIRST_DRAWN.store(true, std::sync::atomic::Ordering::Relaxed);
            let busy = self.in_flight > 0 || !self.world.dirty.is_empty();
            // Columns to mesh: the loaded ones, minus those waiting for a neighbour.
            let cols = self.world.grid.len().saturating_sub(self.world.meshes.waiting_len());
            perf_log::frame(self.started, busy, self.positioned && !self.front.is_open(), cols);
        }
        let since = self.fps_since.elapsed().as_secs_f64();
        if since >= 1.0 {
            let (cols, quads) = r.stats();
            let (va, vu, vt, vo) = r.vram();
            let mo = |b: u64| b as f64 / 1_048_576.0;
            let cpu = r.cpu_ms();
            let gpu_ms = r.gpu_ms();
            self.debug.set_gpu(gpu_ms);
            let gpu = gpu_ms.map_or_else(|| "?".into(), |ms| format!("{ms:.3}"));
            let line = format!(
                "{:.0} fps, CPU {cpu:.3} ms/frame, GPU {gpu} ms/frame, {} columns loaded, {} on GPU ({:.2} M faces), {} drawn ({:.2} M faces), {} meshes in progress, sort {:.3} ms (walk {:.3} ms), VRAM {:.1} MB (regions {:.1} of which {:.1} used, textures {:.1}, other {:.1}), translucent {} columns of which {} unsorted, {} entities drawn, block entities {} of which {} drawn",
                f64::from(self.fps_frames) / since,
                self.world.grid.len(),
                cols,
                quads as f64 / 1e6,
                r.drawn.0,
                r.drawn.1 as f64 / 1e6,
                self.in_flight,
                r.vis_ms,
                r.graph_ms,
                mo(va + vt + vo),
                mo(va),
                mo(vu),
                mo(vt),
                mo(vo),
                r.trans_stats().0,
                r.trans_stats().1,
                r.entities.as_ref().map_or(0, |e| e.drawn),
                self.block_entities.len(),
                r.entities.as_ref().map_or(0, |e| e.blocks.drawn)
            );
            eprintln!("[{:>6.1} s] {line}", self.started.elapsed().as_secs_f64());
            alloc_trace::per_second(since, self.fps_frames);
            // Total of the allocator (Vulkan): everything included (textures, buffers, targets).
            if let Some(rep) = r.allocator_report() {
                eprintln!(
                    "GPU allocator: {:.1} MB allocated, {:.1} MB reserved",
                    mo(rep.total_allocated_bytes),
                    mo(rep.total_reserved_bytes)
                );
                if rc_data::env_flag!("RUSTCRAFT_VRAMDUMP") {
                    let mut a: Vec<_> = rep.allocations.iter().map(|x| (x.size, x.name.as_str())).collect();
                    a.sort_unstable_by_key(|x| std::cmp::Reverse(x.0));
                    for (size, name) in a.iter().take(15) {
                        eprintln!("  {:>8.2} MB  {name}", mo(*size));
                    }
                }
            }
            #[cfg(feature = "memstats")]
            eprintln!(
                "Rust heap: {:.1} MB (world {:.1} MB, render translucents {:.1} MB)",
                memstats::LIVE.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1_048_576.0,
                self.world.grid.iter().map(rc_world::Column::heap_bytes).sum::<usize>() as f64 / 1_048_576.0,
                r.trans_bytes() as f64 / 1_048_576.0
            );
            // Title like the official one ("<name> <version> - <mode>"), name Rustcraft (decided on
            // 12/10/2026); the readings stay in the log above.
            let mode = front::title_mode_key(self.solo.is_some(), self.menus.world.published.is_some());
            let mode = r.gui.lang.map.get(mode).map_or(mode, String::as_str);
            r.window.set_title(&format!("Rustcraft {} - {mode}", front::VERSION_NAME));
            self.fps_frames = 0;
            self.fps_since = Instant::now();
        }
    }

    /// Sky colour of the biome under the camera (without time or blend for now).
    /// Client tick of the block entities (`block-entities.md` § 1.5).
    fn tick_block_entities(&mut self) {
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let air = &self.tables.air;
        let top = self.world.min_y + self.world.height;
        let surface = |x: i32, z: i32| {
            (self.world.min_y..top)
                .rev()
                .find(|&y| !air.get(view.state(x, y, z) as usize).copied().unwrap_or(true))
                .map_or(self.world.min_y, |y| y + 1)
        };
        let mut players: Vec<[f64; 3]> =
            self.entities.map.values().filter(|e| e.kind == "player").map(|e| e.pos).collect();
        if self.positioned && self.env.game_mode != 3 {
            players.push(self.player.pos);
        }
        let mut named: Vec<(u128, [f64; 3])> =
            self.entities.map.values().filter(|e| e.kind == "player").map(|e| (e.uuid, e.pos)).collect();
        if self.positioned && self.env.game_mode != 3 {
            named.push((self.local_uuid, self.player.pos));
        }
        let w = block_entities::TickWorld {
            state: &|x, y, z| view.state(x, y, z),
            surface: &surface,
            players: &players,
            named: &named,
            game_time: self.env.env.clocks.game_time,
            min_y: self.world.min_y,
        };
        let mut emitted = Vec::new();
        self.block_entities.tick(&w, &mut emitted);
        let opts = particles::Options::default();
        for (kind, pos, vel) in emitted {
            self.particles.spawn(kind, &opts, pos, vel);
        }
        self.pistons_due = true;
    }

    /// One game tick in the official order: player physics (with the abilities packet
    /// if it toggles flight), then synchronisation (inputs if they change, sprint, position),
    /// then end of tick. "Player loaded" is only signalled once its chunk is received.
    fn game_tick(&mut self) {
        profiler::push("Keybindings");
        self.debug_tick();
        self.keys_tick();
        profiler::pop_push("gui");
        if self.pause.on {
            self.paused_tick();
            return;
        }
        // Frozen world (`/tick freeze`): entities, textures, particles, weather and clocks stopped.
        let runs = self.exact.rate.begin_tick();
        self.chat.tick();
        if self.positioned {
            self.cooldowns.tick();
        }
        self.commands_tick();
        self.hud_extra.tick();
        self.front.loading_tick(); // loading screen: smoothed progress
        world_list::tick_autosave(self.solo.as_ref().is_some_and(rc_sim::Handle::saving));
        // Music then ambiences (sounds without a packet), before the sound engine (`sounds.md` § 3.8).
        if let Some(a) = self.audio.get() {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let ctx = world_sounds::TickCtx {
                view: &view,
                env: &mut self.env,
                player: self.positioned.then_some(&self.player),
                camera: self.eye,
                dimension: &self.dimension,
                boss_flags: self.hud_extra.boss_flags(),
                // Connection in progress (before the first position): world loading screen.
                loading: !self.positioned,
                paused: false,
            };
            self.world_sounds.tick_end_flash(a, self.shot.pos);
            self.world_sounds.tick(a, ctx);
        }
        self.tick_audio();
        entity_render::set_game_time(self.env.env.clocks.game_time);
        // World thunderstorm: world with weather, thunder > 0.9.
        let weather_ok = self.env.has_sky && !self.dimension.ends_with("the_end");
        entity_render::set_thundering(weather_ok && self.env.env.weather.thunder() > 0.9);
        profiler::pop_push("entities");
        {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            entity_render::water_track(&mut self.entities, &self.data, &self.data.entity_dims, &|x, y, z| {
                view.state(x, y, z)
            });
            // Start of the parrots' tick (frozen world: no tick, nothing forgotten).
            if runs {
                let data = &self.data;
                entity_render::party_track(&mut self.entities, &|j| {
                    data.block_of(view.state(j[0], j[1], j[2])).is_some_and(|b| b.name == "minecraft:jukebox")
                });
            }
        }
        ticking::tick_entities(&mut self.entities, runs);
        if runs {
            // Projectiles, items, orbs, falling blocks, TNT: also simulated client-side between
            // two server resyncs (`projectile_sim.rs`, `entity_motion.rs`).
            let colliders = entity_motion::colliders(&self.entities, None, None);
            let world = GridWorld {
                grid: &self.world.grid,
                min_y: self.world.min_y,
                height: self.world.height,
                t: &self.phys,
                border: &self.weather.border,
                colliders: &colliders,
                movers: Some(&self.block_entities),
                noclip: None,
            };
            let me = rc_physics::body::Follow {
                id: self.player_id,
                pos: self.player.pos,
                eye: self.player.eye_height(),
                spectator: !self.positioned || self.player.spectator,
                dead: self.hud.dead(),
            };
            let players = &self.players;
            let spectator = |u: u128| players.map.get(&u).is_some_and(|p| p.game_mode == 3);
            let (time, lava) = (self.env.env.clocks.game_time, self.player.fast_lava);
            let ps = &mut self.particles;
            projectile_sim::tick(&mut self.entities, &self.phys, &world, me, time, lava, &self.misc.power, ps);
            entity_motion::tick(&mut self.entities, &world, &self.phys, me, lava, &spectator, ps);
        }
        capseq::log_tick(&self.entities);
        // Block entities also stopped when the world is frozen.
        profiler::pop_push("blockEntities");
        if runs {
            self.tick_block_entities();
            self.tick_entity_particles();
        }
        self.exact_tick();
        self.misc_tick(runs);
        // Without an open screen, a dead player (connection while dead) sees the death screen without cause.
        if self.death.is_none() && self.chat.input.is_none() && self.hud.dead() {
            self.open_death(None);
        }
        self.inventory.tick();
        self.creative_tick();
        self.book_tick();
        self.invtest_tick();
        self.special_tick();
        self.bed_tick();
        self.win_tick();
        self.menus_tick();
        self.solo_pause();
        let held = self.inventory.hotbar(self.hud.selected).map(|s| {
            let view = stack::StackRef {
                stack: s,
                info: usize::try_from(s.item).ok().and_then(|i| self.data.items.get(i)),
                ids: &self.component_ids,
                data: &self.data,
                hold: stack::Hold::MAIN,
            };
            (s.item, view.display_name())
        });
        self.hud.tick_held(held);
        if let Some(d) = &mut self.death {
            d.tick();
            // Mouse-free test: click on "Respawn" after 60 ticks.
            if rc_data::env_flag!("RUSTCRAFT_DEATHTEST") && d.ticks() == 60 {
                let _ = self.session.link.send_client_command(0);
            }
        }
        // Following emitters, music notification (`world_fx.rs`).
        if runs {
            self.fx_tick();
            self.entity_events_tick();
        }
        self.bundle_tick();
        self.bundle_sound();
        self.social_toast_tick();
        self.tutorial_tick();
        // Animation of the blocks around the player (`block_anim.rs`), before the particles tick.
        profiler::pop_push("animateTick");
        if runs && self.positioned {
            self.tick_block_anim();
        }
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        profiler::pop_push("level");
        self.env.frozen = !runs;
        self.env.boss_flags = self.hud_extra.boss_flags();
        self.env.hide_flashes = self.options.b("hideLightningFlashes");
        self.env.tick(&view, self.player.eyes_in_water);
        // End lightning started: delayed sound, except in front of the end screen.
        if let Some(angles) = self.env.end_flash_started()
            && self.win.is_none()
        {
            self.world_sounds.queue_end_flash(angles);
        }
        self.weather.ticks += 1;
        if self.weather.ticks.is_multiple_of(20) && rc_data::env_flag!("RUSTCRAFT_WEATHERLOG") {
            let (t, rain) = (self.env.env.clocks.game_time, self.env.env.weather.rain);
            eprintln!("{}, time {t}, rain {rain}", self.weather.debug_line(&view, self.eye));
        }
        // Particles: test emissions, then engine tick (current world and camera).
        profiler::pop_push("particles");
        if self.positioned {
            self.particles.test_emit(self.eye, self.yaw);
        }
        self.particles.set_biomes(&self.palette, self.video.mesh.blend, self.env.plains());
        // Vibration signals: target followed before the tick (fields borrowed one by one).
        let (ents, me) = (&self.entities, (self.player_id, self.player.pos));
        self.particles.track_vibrations(|id| if id == me.0 { Some(me.1) } else { ents.map.get(&id).map(|e| e.pos) });
        let pw = particles::TickWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            player: self.positioned.then_some((self.player.pos, self.player.vel)),
            scope: (self.positioned && self.scoping_first_person()).then_some(self.eye),
        };
        if runs {
            self.particles.tick(&pw, self.eye);
            // Particle sounds (drips, fireworks).
            for s in self.particles.take_sounds() {
                if let Some(a) = self.audio.get() {
                    s.play(a, self.eye);
                }
            }
        }
        // Weather: drops on the ground, rain sound, lightning (`weather-border.md` § 2-3).
        if runs {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let ctx = weather::TickCtx {
                view: &view,
                data: &self.data,
                phys: &self.phys,
                particles: &mut self.particles,
                audio: self.audio.get(),
                entities: &self.entities,
                eye: self.shot.pos,
                rain: self.env.env.weather.rain,
                game_time: self.env.env.clocks.game_time,
                radius: self.options.i("weatherRadius"),
                particle_level: self.options.i("particles"),
            };
            for id in self.weather.tick(ctx) {
                self.entities.map.remove(&id);
            }
            self.env.sky_flash = self.weather.sky_flash > 0 && !self.options.b("hideLightningFlashes");
            self.hud.border_warn = self.weather.border.warning(self.player.pos[0], self.player.pos[2]);
        }
        let mut eye = self.player.pos;
        eye[1] += self.player.eye_height();
        self.hud.tick(self.env.eye_brightness(&view, eye));
        self.video.darkness.tick();
        self.hud.more.tick(&hud_more::TickIn {
            air: self.entities.me.as_ref().and_then(|e| e.meta_int(1)),
            eyes_in_water: self.player.eyes_in_water,
            vehicle: hud_more::vehicle_of(&self.entities),
            frozen: self.entities.me.as_ref().and_then(|e| e.meta_int(7)),
            in_portal: self.positioned
                && self.env.game_mode != 3
                && hud_more::in_portal(&self.data, |x, y, z| view.state(x, y, z), self.player.pos),
            // Keyboard-free test: RUSTCRAFT_JUMPTEST holds the mount jump charge.
            jump: (self.grabbed && self.binds.down(keys::JUMP))
                || rc_data::env_flag!("RUSTCRAFT_JUMPTEST")
                || self.riding.script_jump(),
        });
        self.hud.more.prune(&self.entities);
        if let Some(power) = self.hud.more.take_jump() {
            let _ = self.session.link.send_player_command_data(rc_client::link::action::START_RIDING_JUMP, power);
            self.riding.jump = Some(power);
        }
        self.sun_angle = self.env.sun_angle();
        let link = self.session.link.clone();
        if !self.positioned {
            self.tick_pistons();
            return;
        }
        profiler::pop_push("playerSync");
        // Carried slot: sent at the tick following the change (`hud.md` § 6.2).
        if let Some(slot) = self.hud.take_carried() {
            let _ = link.send_carried_item(slot);
        }
        if !link.is_loaded() {
            // End of the world loading screen (`flow.rs`).
            if !self.world_loaded() {
                self.tick_pistons();
                return;
            }
            let _ = link.send_player_loaded();
            // Keyboard-free test: a message and a command on entering the game.
            // `RUSTCRAFT_CHATTEST=<command>`: this command instead of the time query.
            if let Some(v) = rc_data::env_os!("RUSTCRAFT_CHATTEST") {
                let _ = link.send_chat("Hello from Rustcraft");
                let cmd = v.to_str().filter(|s| s.len() > 1).unwrap_or("time query daytime");
                let _ = link.send_command(cmd);
            }
        }
        // Clicks, mining, use and action keys (`interactions.md`), before the physics.
        self.act_tick();
        let k = |i: usize| self.grabbed && self.binds.down(i);
        let input = Input {
            forward: k(keys::FORWARD),
            backward: k(keys::BACK),
            left: k(keys::LEFT),
            right: k(keys::RIGHT),
            jump: k(keys::JUMP),
            shift: k(keys::SNEAK),
            sprint: k(keys::SPRINT),
        };
        let mut input = if self.free_cam { Input::default() } else { input };
        // Keyboard-free test: RUSTCRAFT_AUTOWALK=<seconds> walks forward (sprint, jump every 2 s).
        if let Some(secs) = self.autowalk
            && self.started.elapsed().as_secs_f64() < secs
        {
            self.autowalk_ticks += 1;
            input.forward = true;
            input.sprint = true;
            input.jump = self.autowalk_ticks % 40 < 2;
            // Turns slowly to meet some terrain.
            self.yaw += 0.5;
        }
        if !self.free_cam {
            self.player.yaw = self.yaw;
            self.player.pitch = self.pitch;
        }
        self.prev_pos = self.player.pos;
        // Piloted vehicle, passengers on their seat; camera elsewhere: motionless (`riding.rs`).
        let free = self.moves_freely() && self.sleeping_pos().is_none();
        self.ride_tick(&mut input);
        // Equipment (elytra, boots), attached rockets, pushing neighbours (`player_env.rs`).
        self.player_env_tick();
        // Boats, shulkers, happy ghasts: solid for the player too.
        let colliders = entity_motion::colliders(&self.entities, None, Some(self.player.pos[1]));
        let world = GridWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            t: &self.phys,
            border: &self.weather.border,
            colliders: &colliders,
            movers: Some(&self.block_entities),
            noclip: None,
        };
        self.player.spectator = self.env.game_mode == 3;
        let sig = if free { self.player.tick(input, &world, &self.phys) } else { Default::default() };
        // Auto jump: the forced jump also goes in the inputs packet.
        input.jump |= sig.auto_jump;
        if let Some(a) = self.audio.get() {
            let moved = std::array::from_fn(|i| self.player.pos[i] - self.prev_pos[i]);
            self.world_sounds.steps(a, &self.data, &self.phys, &world, &self.player, moved);
            self.world_sounds.tick_elytra(a, &self.player);
        }
        // Moving blocks after the player's own movement, before sending its
        // position (the game: entities, block entities, then player synchronisation).
        self.tick_pistons();
        self.tick_camera();
        self.hud.detached = self.camera.view.detached();
        if let Some(t) = &mut self.trace {
            use std::io::Write;
            let (p, v) = (self.player.pos, self.player.vel);
            let _ = writeln!(
                t,
                "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {}",
                p[0],
                p[1],
                p[2],
                v[0],
                v[1],
                v[2],
                u8::from(self.player.on_ground)
            );
        }
        if let Some(f) = sig.flying {
            let _ = link.send_abilities(f);
        }
        if sig.start_fall_flying {
            let _ = link.send_player_command(rc_client::link::action::START_FALL_FLYING);
        }
        if self.last_input != Some(input.bits()) {
            let _ = link.send_input(input.bits());
            self.last_input = Some(input.bits());
        }
        if let Some(on) = sig.sprinting {
            use rc_client::link::action;
            let _ = link.send_player_command(if on { action::START_SPRINTING } else { action::STOP_SPRINTING });
        }
        let p = &self.player;
        if !free {
            // Passenger or camera on another entity (`riding.rs`).
            self.send_moves_exact();
        } else if let Some(m) = self.sync.tick(p.pos, p.yaw, p.pitch, p.on_ground, p.horizontal_collision) {
            let _ = link.send_move(m, p.on_ground, p.horizontal_collision);
        }
        let _ = link.send_tick_end();
    }

    /// Camera and local player after the tick's physics (`hand-camera.md` § 3 to 5).
    fn tick_camera(&mut self) {
        let p = &self.player;
        let (dx, dz) = (p.pos[0] - self.prev_pos[0], p.pos[2] - self.prev_pos[2]);
        let dead = self.hud.dead();
        self.camera.tick(&camera::TickIn {
            eye_height: self.camera_eye_height().unwrap_or_else(|| p.eye_height()),
            flying: p.flying,
            movement_speed: p.attribute("minecraft:movement_speed").unwrap_or(0.1),
            on_ground: p.on_ground,
            swimming: p.swimming,
            dead,
            riding: false,
            moved: dx.hypot(dz),
            speed: p.vel[0].hypot(p.vel[2]),
            yaw: self.yaw,
            pitch: self.pitch,
        });
        self.entities.tick_me(p.pos, self.yaw, self.pitch, p.on_ground, dead);
        // Equipment of the local player: hands and armor taken from the inventory.
        if let Some(me) = &mut self.entities.me {
            // Mouse-free test: RUSTCRAFT_SWINGTEST swings the arm every 2 s.
            if me.age % 40 == 0 && rc_data::env_flag!("RUSTCRAFT_SWINGTEST") {
                me.swing = Some((0, 1, 6, 0));
            }
            me.scale = self.player.attribute("minecraft:scale").unwrap_or(1.0) as f32;
            let inv = &self.inventory;
            me.equipment[0] = inv.hotbar(self.hud.selected).cloned();
            me.equipment[1] = inv.offhand().cloned();
            for (slot, menu) in [(2, 8), (3, 7), (4, 6), (5, 5)] {
                me.equipment[slot] = inv.menu.get(menu).cloned().flatten();
            }
        }
        // Height of the hands (item swap, attack strength).
        let models = self.renderer.as_ref().and_then(|r| r.gui.items.as_ref()).map(|a| &a.models);
        let (ids, data) = (&self.component_ids, &self.data);
        let hand = self.camera.hand.get_or_insert_with(|| hand::HandState::new(data));
        hand.tick(
            [self.inventory.hotbar(self.hud.selected), self.inventory.offhand()],
            self.player.attribute("minecraft:attack_speed").unwrap_or(4.0),
            &|s| hand::swap_props(models, ids, data, s).0,
        );
    }

    /// Camera position of the frame (perspective, third-person pull-back).
    fn place_camera(&mut self, pt: f32) -> camera::Shot {
        if self.free_cam || !self.positioned {
            return camera::Shot { pos: self.eye, yaw: self.yaw, pitch: self.pitch };
        }
        if let Some(s) = self.bed_shot() {
            return s;
        }
        let feet: [f64; 3] =
            std::array::from_fn(|a| self.prev_pos[a] + (self.player.pos[a] - self.prev_pos[a]) * f64::from(pt));
        let scale = self.player.attribute("minecraft:scale").unwrap_or(1.0);
        let distance = if self.hud.dead() {
            4.0
        } else {
            let own = scale * self.player.attribute("minecraft:camera_distance").unwrap_or(4.0).clamp(0.0, 32.0);
            own.max(self.vehicle_camera_distance())
        };
        // Camera on another entity (`set_camera`): its feet and rotation. Boat rotation
        // added to the view: smoothed over the tick (`riding.rs`).
        let yaw = self.yaw - self.riding.turn * (1.0 - pt);
        let (feet, yaw, pitch) = self.camera_pose(pt).unwrap_or((feet, yaw, self.pitch));
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        self.camera.place(&self.data, pt, feet, yaw, pitch, distance, &|x, y, z| view.state(x, y, z))
    }

    /// Death of the local player: death screen, or direct respawn if the server disabled it.
    /// Crosshair aiming (`targeting.md` § 1) from the camera's eyes.
    fn pick(&self) -> pick::Hit {
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let (yr, pr) = (self.yaw.to_radians(), self.pitch.to_radians());
        let dir = [f64::from(-yr.sin() * pr.cos()), f64::from(-pr.sin()), f64::from(yr.cos() * pr.cos())];
        let block_range = self.player.attribute("minecraft:block_interaction_range").unwrap_or(4.5);
        let entity_range = self.player.attribute("minecraft:entity_interaction_range").unwrap_or(3.0);
        let (data, full, held) = (&self.data, self.act.full_shape, self.held_name());
        let shape = |x: i32, y: i32, z: i32| -> pick::Shape<'_> {
            aim_shape(data, full, held, view.state(x, y, z), [x, y, z])
                .map_or_else(|| pick::plain(&[]), |(i, shift)| (data.shape(i), shift))
        };
        // Interaction shape (cauldrons, composter, hopper, scaffolding): aimed face.
        let interaction = |x: i32, y: i32, z: i32| rc_physics::ray::interaction_shape(data, view.state(x, y, z));
        // Active item (in use, otherwise main hand; nothing in spectator) within
        // attack reach: its aiming goes first, player movement included along the gaze.
        let attack = (self.env.game_mode != 3).then(|| self.attack_reach()).flatten();
        let along: f64 = (0..3).map(|i| self.player.vel[i] * dir[i]).sum();
        let attack_max = attack.map_or(0.0, |a| a.max + along.max(0.0));
        let reach = block_range.max(entity_range).max(attack_max) + 8.0;
        let candidates = || self.pick_candidates(reach);
        if let Some(a) = attack {
            let border = self.weather.border.bounds();
            let hit =
                pick::pick_attack(self.eye, dir, a, along, block_range, border, &shape, &interaction, candidates());
            if hit != pick::Hit::Miss {
                return hit;
            }
        }
        pick::pick(self.eye, dir, block_range, entity_range, &shape, &interaction, candidates())
    }

    /// Targetable entities within `reach` of the eyes on each axis: current box and
    /// selection radius.
    pub(crate) fn pick_candidates(&self, reach: f64) -> impl Iterator<Item = pick::Candidate> + '_ {
        let ents = self.renderer.as_ref().and_then(|r| r.entities.as_ref());
        let table = ents.map_or(&[][..], |e| e.dims.as_slice());
        let painting = move |v: i32| ents.and_then(|e| e.painting_size(v));
        self.entities.map.iter().filter_map(move |(&id, e)| {
            // Out of reach: discarded before any kind test.
            let p = e.pos;
            if (0..3).any(|i| (p[i] - self.eye[i]).abs() > reach) {
                return None;
            }
            let pickable = match e.kind.as_str() {
                "player" => self.players.map.get(&e.uuid).is_none_or(|p| p.game_mode != 3),
                "armor_stand" => e.meta_byte(15) & 0x10 == 0,
                // Outside the `redirectable_projectile` tag: never aimed at, in flight or stuck.
                "arrow" | "spectral_arrow" | "trident" => false,
                // Entities hanging on a block: always targetable.
                "painting" | "item_frame" | "glow_item_frame" | "leash_knot" => true,
                "end_crystal" | "interaction" | "falling_block" | "tnt" | "shulker_bullet" | "fireball"
                | "wind_charge" | "breeze_wind_charge" => true,
                k if k.ends_with("boat") || k.ends_with("raft") || k.ends_with("minecart") => true,
                _ => e.class != entities::Kind::Other,
            };
            if !pickable {
                return None;
            }
            // Current box (type size, baby, cubes, scale, interaction, shulker…), inflated
            // by the selection radius: 1 for redirectable projectiles.
            let b = e.bounding_box(table, &painting, debug_render::framed_map(e, &self.component_ids)).1;
            let r =
                if matches!(e.kind.as_str(), "fireball" | "wind_charge" | "breeze_wind_charge") { 1.0 } else { 0.0 };
            Some(pick::Candidate { id, bb: rc_data::Aabb { min: b.min, max: b.max }, radius: r, same_vehicle: false })
        })
    }

    /// Attack reach of the active item (`attack_range`): the one in use, otherwise the main
    /// hand; ranges of creative mode or of the other modes.
    fn attack_reach(&self) -> Option<pick::AttackReach> {
        let stack = match self.act.using.as_ref() {
            Some(u) if u.hand == 1 => self.inventory.offhand(),
            _ => self.inventory.hotbar(self.hud.selected),
        }?;
        let r = item_use::ItemView::new(stack, &self.data, &self.act.ids).attack_range()?;
        let (min, max) = if self.env.game_mode == 1 { (r[2], r[3]) } else { (r[0], r[1]) };
        Some(pick::AttackReach { min: f64::from(min), max: f64::from(max), margin: f64::from(r[4]) })
    }

    /// Id of the item held in the main hand (empty without an item).
    fn held_name(&self) -> &str {
        let item = self.inventory.hotbar(self.hud.selected).and_then(|s| usize::try_from(s.item).ok());
        item.and_then(|i| self.item_names.get(i)).map_or("", String::as_str)
    }

    /// Outline of the aimed block (`targeting.md` § 4): shape (depending on the held item), boxes, corner relative
    /// to the camera origin (block's random offset included), translucent block. No
    /// outline outside the world border; in adventure, only if the held item can break
    /// the block or be placed on it.
    fn outline_target(&self, hit: pick::Hit) -> Option<(u16, Vec<rc_data::Aabb>, [f32; 3], bool)> {
        let pick::Hit::Block { pos, .. } = hit else { return None };
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let state = view.state(pos[0], pos[1], pos[2]);
        let (shape, shift) = aim_shape(&self.data, self.act.full_shape, self.held_name(), state, pos)?;
        let boxes = self.data.shape(shape);
        // Spectator: same test as the crosshair; adventure:
        // predicates `can_break` / `can_place_on` of the main hand's item.
        let allowed = match self.env.game_mode {
            0 | 1 => true,
            3 => self.hud.spectator_aim,
            _ => self.inventory.hotbar(self.hud.selected).is_some_and(|s| {
                let v = item_use::ItemView::new(s, &self.data, &self.act.ids);
                v.can_break(&self.data, &self.act.tags, state) || v.can_place_on(&self.data, &self.act.tags, state)
            }),
        };
        // Air has an empty shape: discarded with it.
        if self.hud.hidden || !allowed || boxes.is_empty() || !self.weather.border.block_within(pos) {
            return None;
        }
        let models = &self.tables.models;
        let translucent = models.states.get(state as usize).is_some_and(|s| {
            s.parts.iter().flat_map(|p| &p.options).any(|&(_, first, count)| {
                models.quads[first as usize..(first + count) as usize]
                    .iter()
                    .any(|q| q.layer == rc_assets::bake::Layer::Translucent)
            })
        });
        let origin = [self.shot.pos[0].floor() as i32, self.shot.pos[2].floor() as i32];
        let corner = [
            (f64::from(pos[0] - origin[0]) + shift[0]) as f32,
            (f64::from(pos[1]) + shift[1]) as f32,
            (f64::from(pos[2] - origin[1]) + shift[2]) as f32,
        ];
        Some((shape, boxes.to_vec(), corner, translucent))
    }

    /// `level_particles` packet (`particles.md` § 1.1): an exact particle if the count is
    /// zero, otherwise Gaussian draws (default) or uniform ones (alternatives).
    fn level_particles(&mut self, p: &rc_proto_v777::entity::LevelParticles) {
        let Some(kind) = self.particles.kind_of(p.particle.kind) else { return };
        let o = particles::Options::from_net(&p.particle, p.force);
        let (d, v) = (p.delta.map(f64::from), p.speed.map(f64::from));
        let always = p.always_visible;
        if p.count == 0 {
            let vel = [v[0] * d[0], v[1] * d[1], v[2] * d[2]];
            self.particles.spawn_ex(kind, &o, p.pos, vel, always);
            return;
        }
        let r = &mut self.world_random;
        for _ in 0..p.count.min(16384) {
            let (pos, vel) = if p.mode == 0 {
                let off = [r.gaussian() * d[0], r.gaussian() * d[1], r.gaussian() * d[2]];
                let vel = [r.gaussian() * v[0], r.gaussian() * v[1], r.gaussian() * v[2]];
                (off, vel)
            } else {
                let off = [f64::from(r.f()) * d[0], f64::from(r.f()) * d[1], f64::from(r.f()) * d[2]];
                let vel = if p.mode == 2 {
                    [v[0] * f64::from(r.f()), v[1] * f64::from(r.f()), v[2] * f64::from(r.f())]
                } else {
                    v
                };
                (off, vel)
            };
            let at = [p.pos[0] + pos[0], p.pos[1] + pos[1], p.pos[2] + pos[2]];
            self.particles.spawn_ex(kind, &o, at, vel, always);
        }
        if o.vib.is_some() {
            // Vibration signal that follows an entity: oriented from its creation.
            let (ents, me) = (&self.entities, (self.player_id, self.player.pos));
            self.particles.track_vibrations(|id| if id == me.0 { Some(me.1) } else { ents.map.get(&id).map(|e| e.pos) });
        }
    }

    /// `sound` and `sound_entity` packets (`sounds.md` § 1.3, 1.4).
    fn play_sound(&mut self, s: rc_proto_v777::play::SoundPacket) {
        let Some(audio) = self.audio.get() else { return };
        let name = match &s.sound {
            rc_proto_v777::play::SoundRef::Named(n) => n.clone(),
            rc_proto_v777::play::SoundRef::Id(i) => {
                match self.data.registry("minecraft:sound_event").and_then(|r| r.entries.get(*i as usize)) {
                    Some(n) => n.clone(),
                    None => return,
                }
            }
        };
        let category = rc_audio::Category::from_id(s.category).unwrap_or(rc_audio::Category::Master);
        let pos = match s.entity {
            None => s.pos,
            Some(id) => {
                // Unknown or silent entity (flag 4 of the metadata): nothing.
                let Some(e) = self.entities.map.get(&id) else { return };
                if e.meta_bool(4) {
                    return;
                }
                Some(e.pos)
            }
        };
        let handle = audio.play(&name, category, pos, s.volume, s.pitch, Some(s.seed));
        if let (Some(h), Some(id)) = (handle, s.entity) {
            self.entity_sounds.push((h, id));
        }
    }

    /// Audio tick: sounds attached to the tracked entities (stopped if the entity disappears or goes quiet).
    fn tick_audio(&mut self) {
        self.watch_audio_devices();
        let Some(audio) = self.audio.get() else { return };
        let ents = &self.entities;
        let me = self.player.pos;
        self.entity_sounds.retain(|&(h, id)| {
            if !audio.is_playing(h) {
                return false;
            }
            match ents.sound_anchor(id, me) {
                Some(p) => {
                    audio.set_position(h, p.map(|v| f64::from(v as f32)));
                    true
                }
                None => {
                    audio.stop_handle(h);
                    false
                }
            }
        });
        audio.tick();
    }

    /// Closing of a screen by the client: `container_close` then local closing (§ 1.2).
    fn close_screen(&mut self) {
        if self.creative_ui.open {
            self.close_creative();
            return;
        }
        if !self.screens.open {
            return;
        }
        let window = self.inventory.container.as_ref().map_or(0, |c| c.window);
        let _ = self.session.link.send_container_close(window);
        self.inventory.container = None;
        self.screens.hide();
        self.grab(true);
    }

    /// Clicks produced by the screen: prediction and sending, in order.
    fn screen_clicks(&mut self, clicks: Vec<screens::Click>) {
        let rules = screens::ItemRules {
            infos: &self.data.items,
            names: &self.item_names,
            ids: &self.component_ids,
            data: &self.data,
        };
        let creative = self.env.game_mode == 1;
        for c in clicks {
            self.recipe_book.crafting_click(screens::current(&self.inventory).0, c.slot);
            self.screens.perform(&mut self.inventory, &rules, creative, &self.session.link, c);
        }
        // Shift + double click: each slot is re-evaluated after the previous clicks.
        while let Some(c) = self.screens.next_sweep(&self.inventory) {
            self.recipe_book.crafting_click(screens::current(&self.inventory).0, c.slot);
            self.screens.perform(&mut self.inventory, &rules, creative, &self.session.link, c);
        }
    }

    /// Cursor in interface units.
    fn gui_mouse(&self) -> Option<(f32, f32)> {
        static FORCED: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
        let forced = FORCED.get_or_init(|| {
            let v = rc_data::env_str!("RUSTCRAFT_INVMOUSE")?;
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        });
        if forced.is_some() {
            return *forced;
        }
        if self.menutest.2.is_some() {
            return self.menutest.2;
        }
        let s = self.renderer.as_ref().map_or(1.0, |r| f64::from(r.gui.scale));
        self.cursor.map(|(x, y)| ((x / s) as f32, (y / s) as f32))
    }

    /// Keyboard-free demonstration (`RUSTCRAFT_INVTEST`): `e` opens the inventory, `chest`
    /// uses the aimed block; then `RUSTCRAFT_INVCLICKS` = "slot:button:mode" separated by
    /// commas, one click every 10 ticks.
    fn invtest_tick(&mut self) {
        let Some(test) = rc_data::env_str!("RUSTCRAFT_INVTEST") else { return };
        if !self.positioned {
            return;
        }
        self.invtest.1 += 1;
        if self.invtest.1 < 320 || !self.invtest.1.is_multiple_of(10) {
            return;
        }
        if self.invtest.0 == 0 {
            // Forced block (`x,y,z`: top face) or aimed block.
            let forced = test.split(',').map(|v| v.trim().parse::<i32>()).collect::<Result<Vec<_>, _>>().ok();
            if test == "e" {
                self.screens.show();
            } else if let Some([x, y, z]) = forced.as_deref() {
                let seq = self.act.next_seq();
                let _ = self.session.link.send_use_item_on([*x, *y, *z], 1, [0.5, 1.0, 0.5], false, seq);
                eprintln!("demo: use of block {x} {y} {z}");
            } else if let pick::Hit::Block { pos, face, at, inside } = self.pick() {
                let seq = self.act.next_seq();
                let rel = [0, 1, 2].map(|a| (at[a] - f64::from(pos[a])) as f32);
                let _ = self.session.link.send_use_item_on(pos, i32::from(face), rel, inside, seq);
                eprintln!("demo: use of block {pos:?}");
            }
            self.invtest.0 = 1;
            return;
        }
        let clicks = rc_data::env_str!("RUSTCRAFT_INVCLICKS").unwrap_or_default();
        let Some(spec) = clicks.split(',').filter(|s| !s.is_empty()).nth(self.invtest.0 - 1) else { return };
        let v: Vec<i32> = spec.split(':').filter_map(|x| x.trim().parse().ok()).collect();
        if let [slot, button, mode] = v[..] {
            let c = screens::Click { slot: slot as i16, button: button as i8, mode };
            eprintln!("demo: {c:?}");
            if mode == 5 && button & 3 == 1 {
                self.screens.drag_sent.push(slot as usize);
            }
            self.screen_clicks(vec![c]);
        }
        self.invtest.0 += 1;
    }

    fn open_death(&mut self, message: Option<rc_text::Component>) {
        self.close_screen();
        self.menus.close_all();
        if self.show_death_screen {
            self.death = Some(death::DeathScreen::new(message, self.hud.hardcore, self.hud.score));
            self.chat.close();
            self.keys.clear();
            self.grab(false);
        } else {
            let _ = self.session.link.send_client_command(0);
        }
    }

    /// Key pressed during chat input (`chat.md` § 4, `commands.md` § 4.2).
    fn chat_key(&mut self, event: &winit::event::KeyEvent) {
        use winit::keyboard::{Key, NamedKey};
        let (ctrl, shift) = (self.mods.control_key() && !self.mods.alt_key(), self.mods.shift_key());
        // Ctrl + A/C/V/X shortcuts: layout letter, otherwise physical key.
        let letter = match &event.logical_key {
            Key::Character(c) => c.chars().next().map(|c| c.to_ascii_lowercase()),
            _ => None,
        }
        .or(match event.physical_key {
            PhysicalKey::Code(KeyCode::KeyA) => Some('a'),
            PhysicalKey::Code(KeyCode::KeyC) => Some('c'),
            PhysicalKey::Code(KeyCode::KeyV) => Some('v'),
            PhysicalKey::Code(KeyCode::KeyX) => Some('x'),
            _ => None,
        });
        let k = match &event.logical_key {
            Key::Named(NamedKey::Escape) => commands::Key::Escape,
            Key::Named(NamedKey::Enter) => commands::Key::Enter,
            Key::Named(NamedKey::Tab) => commands::Key::Tab { shift },
            Key::Named(NamedKey::Backspace) if ctrl => commands::Key::BackspaceWord,
            Key::Named(NamedKey::Backspace) => commands::Key::Backspace,
            Key::Named(NamedKey::Delete) if ctrl => commands::Key::DeleteWord,
            Key::Named(NamedKey::Delete) => commands::Key::Delete,
            Key::Named(NamedKey::Home) => commands::Key::Home { extend: shift },
            Key::Named(NamedKey::End) => commands::Key::End { extend: shift },
            Key::Named(NamedKey::ArrowLeft) => commands::Key::Move { dir: -1, word: ctrl, extend: shift },
            Key::Named(NamedKey::ArrowRight) => commands::Key::Move { dir: 1, word: ctrl, extend: shift },
            _ if ctrl && letter == Some('a') => commands::Key::SelectAll,
            _ if ctrl && letter == Some('c') => commands::Key::Copy,
            _ if ctrl && letter == Some('v') => commands::Key::Paste,
            _ if ctrl && letter == Some('x') => commands::Key::Cut,
            Key::Named(NamedKey::ArrowUp) => commands::Key::Up,
            Key::Named(NamedKey::ArrowDown) => commands::Key::Down,
            Key::Named(NamedKey::PageUp) => commands::Key::PageUp,
            Key::Named(NamedKey::PageDown) => commands::Key::PageDown,
            _ => match widgets::typed_text(event, self.mods) {
                Some(t) => commands::Key::Text(t.to_owned()),
                None => return,
            },
        };
        self.chat_input(k);
    }

    /// Input field key (keyboard or test script).
    fn chat_input(&mut self, k: commands::Key) {
        let outcome = self.cmd.key(&mut self.chat, k);
        self.commands_send();
        match outcome {
            commands::Outcome::None => {}
            commands::Outcome::Empty if self.bed.in_bed_chat => {
                // Chat in bed: Enter on an empty field only empties it (screen kept).
                self.chat.reopen_after_send();
                self.commands_refresh();
            }
            commands::Outcome::Close | commands::Outcome::Empty => {
                self.cmd.close(&mut self.chat);
                self.chat.close();
                self.grab(true);
            }
            commands::Outcome::Send { command, text } => {
                let link = &self.session.link;
                let _ = if command {
                    link.send_command_args(&text, &self.cmd.signable_args(&text))
                } else {
                    link.send_chat(&text)
                };
                if self.bed.in_bed_chat {
                    // Chat in bed: sent without leaving the screen (mouse still free).
                    self.chat.reopen_after_send();
                    self.commands_refresh();
                    return;
                }
                self.cmd.close(&mut self.chat);
                self.chat.close();
                self.grab(true);
            }
        }
    }

    /// Pause menu (`option-menus.md` § 2.1): mouse released, game keys released.
    fn open_pause(&mut self, keyboard: bool) {
        if self.menus.is_open() {
            return;
        }
        self.menus.open(menus::Id::Pause, keyboard);
        self.keys.clear();
        self.grab(false);
    }

    /// Writing of `options.txt`, then `client_information` if the information changed
    /// since the last send (§ 4.4).
    fn save_options(&mut self) {
        if let Err(e) = self.options.save(&self.options_path) {
            eprintln!("options: writing {}: {e}", self.options_path.display());
        }
        let info = self.options.client_info();
        if info != self.last_info {
            let _ = self.session.link.send_client_information(&info);
            self.last_info = info;
        }
    }

    fn menu_actions(&mut self, actions: Vec<menus::Action>) {
        for a in actions {
            match a {
                // Back to the open chat (restrictions screen): mouse left free.
                menus::Action::Resume if self.chat.is_open() => {}
                menus::Action::Resume => self.grab(true),
                // Closing on the client side, back to the server list (`flow.rs`).
                menus::Action::Disconnect => self.disconnect_to_menu(),
                menus::Action::Changed => self.apply_options(),
                // Sound preview of the volume sliders: only outside the world (title screen).
                menus::Action::SoundPreview(c, v) => {
                    if self.front.is_open()
                        && let Some(a) = self.audio.get()
                    {
                        a.preview(c, v);
                    }
                }
                menus::Action::Save => self.save_options(),
                menus::Action::ResourcePacks => self.open_pack_screen(),
                menus::Action::Advancements(kb) => self.open_advancements(true, kb),
                menus::Action::Stats(kb) => self.open_stats(kb),
                menus::Action::Language(code) => {
                    let read = |p: &str| self.assets.read(p);
                    let chosen =
                        if code == "en_us" { None } else { read(&format!("assets/minecraft/lang/{code}.json")) };
                    let mut lang = rc_text::Lang::load(
                        &read("assets/minecraft/lang/en_us.json").unwrap_or_default(),
                        chosen.as_deref(),
                        read("assets/minecraft/lang/deprecated.json").as_deref(),
                    );
                    lang.bidirectional = self.menus.languages.iter().any(|l| l.code == code && l.bidirectional);
                    if let Some(r) = &mut self.renderer {
                        r.gui.lang = lang;
                    }
                }
                other => self.world_menu_action(other),
            }
        }
    }

    /// Effects of the settings the engine can apply (`option-menus.md` § 3); the others
    /// are only recorded. Field of view, view effects and tilt: in
    /// `Options` until the camera module exists.
    fn apply_options(&mut self) {
        let o = &self.options;
        // Engine still being built with these volumes: nothing to do, without joining.
        let v = audio_slot::volumes(o);
        if let Some(a) = self.audio.for_volumes(&v) {
            for c in rc_audio::Category::ALL {
                a.set_volume(c, v[c as usize]);
            }
        }
        self.env.gamma = Some(o.f("gamma") as f32);
        let clouds = o.s("renderClouds") != "false";
        self.env.clouds = Some((clouds, o.i("cloudRange"), o.s("renderClouds") == "fast"));
        self.chat.apply_options(o);
        rc_client::chat::set_only_secure(o.b("onlyShowSecureChat"));
        rc_client::chat::set_visibility(o.i("chatVisibility"));
        rc_client::chat::set_hide_matched_names(o.b("hideMatchedNames"));
        rc_text::set_keybinds(options::KEYS.iter().map(|k| (k.0.to_owned(), o.s(&format!("key_{}", k.0)).to_owned())));
        self.hud.no_vignette = !o.b("vignette");
        self.debug.set_reduced(None, Some(o.b("reducedDebugInfo")));
        self.particles.level = o.i("particles").clamp(0, 2) as u8;
        self.screens.set_advanced(o.b("advancedItemTooltips"));
        self.camera.settings = camera::CameraSettings {
            fov: o.fov() as f32,
            fov_effect_scale: o.f("fovEffectScale") as f32,
            bob_view: o.b("bobView"),
            damage_tilt: o.f("damageTiltStrength") as f32,
        };
        if let Some(r) = &mut self.renderer {
            r.gui.scale_option = o.i("guiScale").max(0) as u32;
            gui::set_force_unicode(o.b("forceUnicodeFont"));
            // Test client in the background: always windowed (a hidden fullscreen window
            // presents no frame).
            // Test clients: never fullscreen from `options.txt` (winit enables a fullscreen
            // window, which would cover the user's screen and take focus).
            let full = o.b("fullscreen") && !launch::background();
            if r.window.fullscreen().is_some() != full {
                r.window.set_fullscreen(full.then_some(winit::window::Fullscreen::Borderless(None)));
            }
        }
        self.apply_video_options();
    }

    /// Menu tick: pause after 500 ms without focus, click sounds, demonstration.
    fn menus_tick(&mut self) {
        let lost =
            self.unfocused_at.is_some_and(|t| t.elapsed().as_millis() > 500) && self.options.b("pauseOnLostFocus");
        // Automatic captures: no pause if the window loses focus.
        if lost && self.positioned && self.capture.is_none() && !self.quit_after_capture && !launch::background() {
            self.unfocused_at = None;
            if !self.screens.open && self.death.is_none() && self.chat.input.is_none() {
                self.open_pause(false);
            }
        }
        self.progress_tick();
        self.world_menus_tick();
        let deferred = self.menus.tick(&mut self.options);
        self.menu_actions(deferred);
        let clicks = std::mem::take(&mut self.menus.clicks);
        if let Some(a) = self.audio.get() {
            for _ in 0..clicks {
                a.play(widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
            }
        }
        self.menutest_tick();
    }

    /// Keyboard-free demonstration: `RUSTCRAFT_MENUTEST=pause|options|video|controls` opens
    /// the screen 12 s after arrival, then `RUSTCRAFT_MENUCLICKS` = actions separated by
    /// commas, one every 10 ticks: `x:y` left click (GUI units), `m:x:y` mouse
    /// only, `d:x0:y0:x1:y1` drag, `w:x:y:notches` wheel, `k:<key>` (Tab, Down, Up,
    /// Left, Right, Enter, Escape).
    fn menutest_tick(&mut self) {
        let Some(test) = rc_data::env_str!("RUSTCRAFT_MENUTEST") else { return };
        if test == "title" {
            self.titletest_tick();
            return;
        }
        if !self.positioned {
            return;
        }
        self.menutest.1 += 1;
        if self.menutest.1 < 240 || !self.menutest.1.is_multiple_of(10) {
            return;
        }
        let Some(r) = &self.renderer else { return };
        if self.menutest.0 == 0 {
            self.open_pause(false);
            if let Some(id) = menus::Id::test_path(test) {
                for s in id {
                    self.menus.open(s, false);
                }
            }
            self.menutest.0 = 1;
            return;
        }
        let spec = rc_data::env_str!("RUSTCRAFT_MENUCLICKS").unwrap_or_default();
        let Some(step) = spec.split(',').filter(|s| !s.is_empty()).nth(self.menutest.0 - 1) else { return };
        self.menutest.0 += 1;
        eprintln!("menu demo: {step}");
        let p: Vec<&str> = step.split(':').collect();
        let f = |s: &str| s.trim().parse::<f32>().unwrap_or(0.0);
        let gui = &r.gui;
        let mut actions = Vec::new();
        match p[..] {
            [x, y] if x != "k" => {
                let m = (f(x), f(y));
                self.menutest.2 = Some(m);
                actions = self.menus.press(gui, &mut self.options, m, 0, false);
                actions.extend(self.menus.release(gui, &mut self.options));
            }
            ["m", x, y] => {
                self.menutest.2 = Some((f(x), f(y)));
                actions = self.menus.motion(gui, &mut self.options, (f(x), f(y)));
            }
            ["d", x0, y0, x1, y1] => {
                actions = self.menus.press(gui, &mut self.options, (f(x0), f(y0)), 0, false);
                actions.extend(self.menus.motion(gui, &mut self.options, (f(x1), f(y1))));
                actions.extend(self.menus.release(gui, &mut self.options));
                self.menutest.2 = Some((f(x1), f(y1)));
            }
            ["w", x, y, n] => {
                self.menutest.2 = Some((f(x), f(y)));
                actions = self.menus.wheel(gui, &mut self.options, (f(x), f(y)), f64::from(f(n)));
            }
            ["k", key] => {
                let k = match key {
                    "Tab" => KeyCode::Tab,
                    "Down" => KeyCode::ArrowDown,
                    "Up" => KeyCode::ArrowUp,
                    "Left" => KeyCode::ArrowLeft,
                    "Right" => KeyCode::ArrowRight,
                    "Enter" => KeyCode::Enter,
                    _ => KeyCode::Escape,
                };
                actions = self.menus.key(gui, &mut self.options, k, self.mods);
            }
            _ => {}
        }
        self.menu_actions(actions);
    }

    fn grab(&mut self, on: bool) {
        // Back in the game: the sounds frozen by the pause resume (`pause.rs`).
        if on && let Some(a) = self.audio.get() {
            a.resume();
        }
        let Some(r) = &self.renderer else { return };
        if on && !launch::background() {
            let _ = r
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| r.window.set_cursor_grab(CursorGrabMode::Confined));
        } else {
            let _ = r.window.set_cursor_grab(CursorGrabMode::None);
        }
        r.window.set_cursor_visible(!on || launch::background());
        self.keys_grab(on);
        self.grabbed = on;
    }
}

impl App {
    /// Texture animation (blocks, item sprites, entity textures including the flames of
    /// burning entities, interface sprites) like the game: a single step
    /// per frame that owes at least one tick, played before those ticks (not one per tick: under 20 frames
    /// per second, the animations slow down), stopped when the world is frozen (`/tick freeze`,
    /// unless stepping is requested). It continues during pause, like in the game.
    fn animate_textures(&mut self) {
        let rate = &self.exact.rate;
        if rate.frozen && rate.steps <= 0 {
            return;
        }
        for (c, a) in self.clocks.iter_mut().zip(&self.models.animations) {
            c.tick(a);
        }
        sprite_anim::advance_clock();
        if let Some(r) = &mut self.renderer {
            if let Some(er) = r.entities.as_mut() {
                er.tick_textures();
            }
            if let Some(items) = r.gui.items.as_mut() {
                items.tick_textures(&r.queue);
            }
            r.animate(self.clocks.iter().zip(&self.models.animations).map(|(c, a)| {
                let (cur, next, milli) = c.frame(a);
                (a.texture, cur, next, milli)
            }));
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let window = Arc::new(el.create_window(launch::window_attributes()).expect("window"));
        boot::mark("window");
        launch::center_window(&window, el);
        launch::show_background(&window);
        launch::set_icon(&window, &self.assets);
        present_watch::set_window(&window);
        present_watch::enter(present_watch::Stage::Frame);
        self.build_renderer(window);
        present_watch::leave();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        self.video.on_window_event(&event);
        // Held keys, activity, global keys (fullscreen, capture): `keys.rs`.
        if self.keys_observe(&event) {
            return;
        }
        // Resource reload (`reload.rs`): the loading screen takes the input.
        if self.reload_blocks_input(&event)
            || self.links_window_event(&event)
            || self.pack_prompt_window_event(&event)
            || self.pack_screen_window_event(&event)
        {
            return;
        }
        if !self.menus.is_open() && self.dialog_window_event(&event) {
            return;
        }
        if self.front_window_event(&event) {
            return;
        }
        if self.progress_window_event(&event) {
            return;
        }
        if !self.menus.is_open() && self.special_window_event(&event) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                if self.quit_allowed() {
                    el.exit();
                }
            }
            WindowEvent::Resized(s) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(s.width, s.height);
                }
            }
            // F3 and F3 + … keys (`debug-f3.md` § 6), before the screens.
            WindowEvent::KeyboardInput { ref event, .. } if self.debug_key_event(event) => {}
            // Debug options screen (F3+F6): the mouse goes back to it.
            ref e @ (WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. })
                if self.debug.options_open() =>
            {
                self.debug_options_mouse(e);
            }
            // Text validated by the IME: typing in the field of the debug options screen.
            ref e @ WindowEvent::Ime(_) if self.debug.options_open() => {
                if let (Some(t), Some(s)) = (ime::commit(e), &mut self.debug.options_screen) {
                    let mut ev = Vec::new();
                    s.panel.typed(t, &mut ev);
                    self.debug_options_events(ev);
                }
            }
            // Recipe book: clicks and typing before the container screen.
            ref e @ (WindowEvent::MouseInput { .. } | WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_))
                if self.screens.open
                    && self.death.is_none()
                    && self.chat.input.is_none()
                    && (self.book_mouse(e) || self.book_key(e)) => {}
            // Creative inventory: its inputs first.
            ref e
                if self.creative_ui.open
                    && self.death.is_none()
                    && self.chat.input.is_none()
                    && self.creative_event(e) => {}
            // End screen, chat in bed, spectator menu (`win_screen.rs`, `bed.rs`, `spectator.rs`).
            ref e if self.win_key(e) || self.bed_key(e) || self.bed_mouse(e) || self.spectator_event(e) => {}
            // Death screen: neither game keyboard nor Esc.
            WindowEvent::KeyboardInput { event, .. } if self.death.is_some() => self.death_key(&event),
            // Menus (pause, options): all the input goes to them.
            WindowEvent::KeyboardInput { event, .. } if self.menus.is_open() => {
                if event.state != ElementState::Pressed {
                    if let PhysicalKey::Code(k) = event.physical_key {
                        self.menus.key_release(k);
                    }
                    return;
                }
                let Some(r) = &self.renderer else { return };
                let mut actions = Vec::new();
                let key = if self.menus.waiting_key() {
                    widgets::physical_key(&event)
                } else {
                    widgets::edit_key(&event, self.mods)
                };
                if let Some(k) = key {
                    actions = self.menus.key(&r.gui, &mut self.options, k, self.mods);
                }
                if let Some(t) = widgets::typed_text(&event, self.mods) {
                    actions.extend(self.menus.typed(&r.gui, &mut self.options, t));
                }
                self.menu_actions(actions);
            }
            // Text validated by the IME: like typing in the menus field.
            ref e @ WindowEvent::Ime(_) if self.menus.is_open() => {
                if let (Some(r), Some(t)) = (&self.renderer, ime::commit(e)) {
                    let actions = self.menus.typed(&r.gui, &mut self.options, t);
                    self.menu_actions(actions);
                }
            }
            WindowEvent::MouseInput { state, button, .. } if self.menus.is_open() => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return };
                let actions = if state == ElementState::Pressed {
                    self.menus.press(&r.gui, &mut self.options, m, b, self.mods.shift_key())
                } else {
                    self.menus.release(&r.gui, &mut self.options)
                };
                self.menu_actions(actions);
            }
            WindowEvent::MouseWheel { delta, .. } if self.menus.is_open() => {
                let y = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(y),
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return };
                let actions = self.menus.wheel(&r.gui, &mut self.options, m, y);
                self.menu_actions(actions);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if self.menus.is_open()
                    && let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse())
                {
                    let actions = self.menus.motion(&r.gui, &mut self.options, m);
                    self.menu_actions(actions);
                }
                if self.screens.open
                    && let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse())
                {
                    let rules = screens::ItemRules {
                        infos: &self.data.items,
                        names: &self.item_names,
                        ids: &self.component_ids,
                        data: &self.data,
                    };
                    self.screens.motion(&r.gui, &self.inventory, m, &rules);
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                create_world::set_alt_down(self.mods.alt_key());
            }
            WindowEvent::KeyboardInput { event, .. } if self.screens.open && self.chat.input.is_none() => {
                // Repeats do not go to the screens.
                if event.state != ElementState::Pressed || event.repeat {
                    return;
                }
                let PhysicalKey::Code(k) = event.physical_key else { return };
                let (digit, drop, close) = self.screen_key_actions(k);
                if close {
                    self.close_screen();
                    return;
                }
                // Crafter: disabled slot re-enabled by the swap key (before the swap).
                if let (Some(d), Some(m)) = (digit, self.gui_mouse()) {
                    self.crafter_swap_key(m, d);
                }
                let click = match (&self.renderer, self.gui_mouse()) {
                    (Some(r), Some(m)) if digit.is_some() || drop.is_some() => {
                        self.screens.key(&r.gui, &self.inventory, m, digit, drop)
                    }
                    _ => None,
                };
                if let Some(c) = click {
                    self.screen_clicks(vec![c]);
                }
            }
            WindowEvent::MouseInput { state, button, .. } if self.screens.open && self.death.is_none() => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return };
                let shift = self.mods.shift_key();
                let clicks = if state == ElementState::Pressed {
                    self.screens.press(&r.gui, &self.inventory, b, m, shift, self.env.game_mode == 1)
                } else {
                    self.screens.release(&r.gui, &self.inventory, b, m, shift)
                };
                self.screen_clicks(clicks);
            }
            // In-game buttons (captured mouse): handled at the tick (`interact.rs`).
            WindowEvent::MouseInput { state, button, .. }
                if self.chat.input.is_none() && self.grabbed && self.death.is_none() =>
            {
                self.game_mouse(state, button);
            }
            WindowEvent::CursorLeft { .. } => self.cursor = None,
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                if self.death.is_some() =>
            {
                let s = self.renderer.as_ref().map_or(1.0, |r| f64::from(r.gui.scale));
                let action = match (self.cursor, &mut self.death) {
                    (Some((x, y)), Some(d)) => d.click((x / s) as f32, (y / s) as f32),
                    _ => None,
                };
                self.death_action(action);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Chat input open: the keyboard goes to the field.
                if self.chat.input.is_some() {
                    if event.state == ElementState::Pressed {
                        self.chat_key(&event);
                    }
                    return;
                }
                // Rebindable game keys (`keys.rs`): Esc, actions, key state.
                self.game_keyboard(&event);
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                if self.chat.input.is_some() =>
            {
                if let Some(m) = self.gui_mouse() {
                    self.cmd.click(&mut self.chat, m);
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                if self.chat.input.is_none() =>
            {
                // Free mouse: the click captures it (without a game action).
                self.grab(true)
            }
            WindowEvent::MouseWheel { delta, .. } if self.chat.input.is_some() => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => (p.y / 20.0) as f32,
                };
                if let Some(m) = self.gui_mouse()
                    && self.cmd.wheel(m, lines)
                {
                    return;
                }
                let step = if self.keys.contains(&KeyCode::ShiftLeft) { 1 } else { 7 };
                self.chat.scroll(lines.signum() as i32 * step);
            }
            // Container screen: the wheel picks a bundle's item, never the bar.
            WindowEvent::MouseWheel { delta, .. } if self.screens.open => self.bundle_wheel(delta),
            WindowEvent::MouseWheel { delta, .. } => self.game_scroll(delta),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.grab(false);
                self.unfocused_at = Some(Instant::now());
            }
            WindowEvent::Focused(true) => self.unfocused_at = None,
            WindowEvent::RedrawRequested => {
                // Frame zones for the F3+1 and F3+L pie chart (`profiler.rs`).
                present_watch::enter(present_watch::Stage::Frame);
                self.profiler_frame_start();
                self.frame();
                // End of loading (equivalent of the game's end of loading): startup successful.
                use std::sync::atomic::Ordering::Relaxed;
                if FIRST_DRAWN.load(Relaxed) && !STARTED_CLEANLY.swap(true, Relaxed) {
                    self.options.set("startedCleanly", options::Val::B(true));
                    self.save_options();
                }
                self.profiler_frame_end();
                present_watch::leave();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        // Accumulated then applied to the frame (`keys.rs` `mouse_frame`: sensitivity, cinematic).
        if let DeviceEvent::MouseMotion { delta } = event {
            self.mouse_motion(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.exit_requested {
            el.exit();
            return;
        }
        self.video_tick();
        // Next frame, at the allowed pace (`keys.rs` `pace_frames`).
        self.pace_frames(el);
    }
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
    // Before any large allocation: allocator pages committed on demand.
    let commit = debug_sys::commit_on_demand();
    alloc_trace::init();
    // Panic of any thread (integrated server included): crash report in the game's format.
    crash_report::install_hook();
    // Java command line of the official launcher ("Java executable" = rustcraft.exe): everything
    // before the main class is ignored; `-version` gets a Java answer.
    let args = match launch::split_command(std::env::args().skip(1).collect()) {
        launch::Command::JavaVersion(v) => {
            println!("{v}");
            eprintln!("{v}");
            return;
        }
        launch::Command::Game(a) => flow::Args::parse(a.into_iter()),
    };
    // Several quick play options: the game refuses to start.
    if args.launch.quick_play_count > 1 {
        eprintln!("quick play: only one --quickPlay… option is allowed");
        std::process::exit(1);
    }
    // Account and window passed by a launcher (`launch.rs`); token in memory only.
    if let Some(s) = args.launch.summary() {
        eprintln!("{s}");
    }
    match commit {
        Some(on) => eprintln!("[opt] allocator pages committed on demand: {on}"),
        None => eprintln!("[opt] allocator pages: user's MIMALLOC_PAGE_COMMIT_ON_DEMAND"),
    }
    if let Some(a) = args.launch.account() {
        rc_client::auth::set_account(a);
    }
    // Read-only account services started at startup, like the game (`session.rs`).
    rc_client::session::configure(rc_client::session::Config {
        proxy: args.launch.proxy.clone(),
        game_dir: Some(servers::game_dir()),
        disable_multiplayer: args.launch.disable_multiplayer,
        disable_chat: args.launch.disable_chat,
    });
    // `--demo`: the worlds opened by this client are in demo mode (integrated server).
    rc_sim::demo::set_launch_demo(args.launch.demo);
    rc_client::session::start_discovery();
    rc_client::session::start_account_services();
    launch::set_window_size(&args.launch);

    let threads = pool_threads();
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("pool-{i}"))
        .start_handler(|_| {
            // Pool at normal priority (`RUSTCRAFT_POOLPRIO=normal` to compare).
            if rc_data::env_str!("RUSTCRAFT_POOLPRIO") != Some("normal") {
                debug_sys::lower_thread_priority();
            }
        })
        .build_global()
        .expect("compute pool");

    let started = Instant::now();
    boot::start();
    boot::mark("entering main");
    let data = Arc::new(rc_data::load_or_prepare("26.3", |s| eprintln!("data: {s}")).unwrap_or_else(|e| {
        eprintln!("version data: {e}");
        std::process::exit(1);
    }));
    memstats::step("version data");
    boot::mark("version data (data.bin)");
    // Settings (`options.txt` of the game folder) read before the resources (chosen packs, language).
    let options_path = servers::game_dir().join("options.txt");
    let mut opts = options::Options::load(&options_path);
    // "startedCleanly" safeguard of the game: set back to true at the first frame drawn.
    opts.begin_startup(rc_data::env_flag!("RUSTCRAFT_BACKEND"));
    let _ = opts.save(&options_path);
    gpu_backend::set_startup_preference(opts.s("preferredGraphicsBackend"));
    // Offline username: `--username`, otherwise the last one used (option `lastUsername`).
    if let Some(u) = &args.username
        && opts.set("lastUsername", options::Val::S(u.clone()))
    {
        let _ = opts.save(&options_path);
    }
    let name = match &args.direct {
        Some((_, _, n)) => n.clone(),
        None if !opts.s("lastUsername").is_empty() => opts.s("lastUsername").to_owned(),
        None => "Rustcraft".to_owned(),
    };
    // Player resources: chosen packs on top of the jar (`resources.rs`).
    let mut pack_stack = pack_stack::PackStack::startup(&mut opts, &options_path);
    boot::mark("options");
    // Audio (`sounds.md`): engine built in the background as soon as the stack is indexed, on the
    // chosen device, joined at first use (`audio_slot.rs`, `RUSTCRAFT_OPT_AUDIOJOIN`).
    let no_audio = rc_data::env_flag!("RUSTCRAFT_NOAUDIO");
    let aopt = audio_slot::opt();
    let (sound_device, sound_volumes) = (opts.s("soundDevice").to_owned(), audio_slot::volumes(&opts));
    let mut audio = audio_slot::Audio::off();
    let (server_paths, player_paths) = pack_stack.split_paths();
    let (loaded, dropped) =
        resources::load_recovering(&data, &server_paths, &player_paths, opts.s("lang"), &mut |assets| {
            if !no_audio && aopt.join {
                // Called for each stack tried (`AUDIOEARLY`): the last one is the one retained.
                audio.respawn(
                    assets.clone(),
                    &sound_device,
                    sound_volumes,
                    aopt.device1,
                    rc_audio::AudioEngine::with_device,
                );
            }
        });
    // Load failed with the chosen packs: removed from the settings, like the game.
    let pack_load_failed = dropped != resources::Dropped::Nothing;
    if pack_load_failed {
        pack_stack.drop_after_failure(dropped == resources::Dropped::All, &mut opts);
        let _ = opts.save(&options_path);
    }
    let resources::Loaded {
        assets,
        models,
        textures,
        tables,
        colormaps,
        sky_assets,
        panorama_assets,
        splashes,
        gui_assets,
        entity_textures,
        particle_assets,
        weather_images,
        languages,
    } = loaded;
    let phys = rc_physics::Tables::new(&data);
    let light = light_engine::LightTables::new(&data);
    boot::mark("physics and light");
    let attr_names = data.registry("minecraft:attribute").map(|r| r.entries.clone()).unwrap_or_default();
    // Old path (`AUDIOJOIN=0`): engine built here (output opened in the background or not depending on
    // `AUDIOFOND`).
    if !no_audio && !aopt.join {
        let t = Instant::now();
        let a = if aopt.device1 {
            audio_slot::build(assets.clone(), &sound_device, &sound_volumes, true, rc_audio::AudioEngine::with_device)
        } else {
            rc_audio::AudioEngine::new(assets.clone())
        };
        eprintln!("audio: ready in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        audio = audio_slot::Audio::ready(a);
    }
    boot::mark("audio");
    let state_bits = data.state_bits() as u8;
    memstats::step("tables (mesher, physics, colours)");
    let local_uuid = launch::profile(&name).0;
    let local_name = name.clone();
    let last_info = opts.client_info();
    // Direct connection (arguments) as before, otherwise title screen without a session.
    let session = match args.direct.clone() {
        Some((host, port, name)) => {
            let (uuid, token) = launch::profile(&name);
            let cfg =
                Config { host, port, name, info: last_info.clone(), uuid: Some(uuid), token, ..Default::default() };
            rc_client::connect(cfg, data.clone())
        }
        None => Session::offline(data.clone()),
    }
    .unwrap_or_else(|e| {
        eprintln!("connection: {e}");
        std::process::exit(1);
    });
    let mut front = front::Front::new(splashes, name.clone());
    front.in_world = args.direct.is_some();
    front.direct = front.in_world;
    if !front.in_world {
        front.open_title(&opts, true);
    }
    let (meshes_tx, meshes_rx) = crossbeam_channel::unbounded();
    let mut app = App {
        session,
        tables,
        clocks: vec![rc_assets::AnimationClock::default(); models.animations.len()],
        shade: rc_mesh::wire::SHADE_DEFAULT,
        video: video::VideoState::default(),
        models,
        textures,
        sky_assets,
        panorama_assets,
        gui_assets: Some(gui_assets),
        entity_textures: Some(entity_textures),
        pending_registries: Vec::new(),
        creative_rec: Default::default(),
        creative_ui: Default::default(),
        hotbars: Default::default(),
        recipe_book: recipe_book::RecipeBook::new(),
        toasts: Default::default(),
        fx: Default::default(),
        ev: Default::default(),
        block_anim: block_anim::BlockAnim::new(&data),
        packs: reload::Packs::new(pack_stack, pack_load_failed),
        progress: Default::default(),
        players: players::PlayerList::default(),
        skins: skins::Skins::new(rc_data::cache_root()),
        local_uuid,
        tab_list: tablist::TabList::default(),
        scores: scoreboard::Scoreboard::default(),
        audio,
        entity_sounds: Vec::new(),
        world_random: level_events::WorldRandom::default(),
        world_sounds: world_sounds::WorldSounds::default(),
        local_name,
        hud_extra: hud_extra::HudExtra::new(),
        online_mode: false,
        entities: entities::Entities {
            type_names: data.registry("minecraft:entity_type").map(|r| r.entries.clone()).unwrap_or_default(),
            dims: data.entity_dims.clone(),
            ..Default::default()
        },
        particles: particles::Particles::new(particle_assets, &data),
        chat: chat::Chat::default(),
        cmd: commands::CommandUi::with_data(&data),
        hud: hud::Hud::default(),
        cooldowns: Default::default(),
        inventory: inventory::PlayerInventory::default(),
        component_ids: stack::ComponentIds::new(&data),
        screens: screens::Screens::new(rc_proto_v777::container::ComponentHasher {
            components: data.registry("minecraft:data_component_type").map(|r| r.entries.clone()).unwrap_or_default(),
            enchantments: Vec::new(),
            potions: data.registry("minecraft:potion").map(|r| r.entries.clone()).unwrap_or_default(),
        }),
        mods: Default::default(),
        invtest: (0, 0),
        act: interact::Interact::new(&data),
        item_names: data.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default(),
        dimension: String::new(),
        spawn: None,
        last_death: None,
        sun_angle: 0.0,
        death: None,
        show_death_screen: true,
        win: None,
        spectator: Default::default(),
        bed: Default::default(),
        cursor: None,
        block_entities: block_entities::BlockEntities::new(data.clone()),
        tech: tech_blocks::TechBlocks::new(&data),
        piston_replay: rc_sim::PistonReplay::new(data.clone()),
        pistons_due: false,
        data,
        colormaps,
        palette: Arc::new(Vec::new()),
        env: environ::EnvState::default(),
        biome_names: Vec::new(),
        world: World {
            grid: ChunkGrid::new(16),
            min_y: 0,
            height: 384,
            state_bits,
            versions: HashMap::new(),
            dirty: HashMap::new(),
            meshes: Default::default(),
            counter: 0,
            hm_rules: Vec::new(),
        },
        renderer: None,
        meshes_tx,
        meshes_rx,
        mesh_pending: Vec::new(),
        in_flight: 0,
        eye: [0.0, 100.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        placed: false,
        keys: HashSet::new(),
        binds: Default::default(),
        grabbed: false,
        last: Instant::now(),
        fps_frames: 0,
        fps_since: Instant::now(),
        started,
        status: String::new(),
        capture: rc_data::env_str!("RUSTCRAFT_CAPTURE").and_then(|v| {
            let (path, secs) = v.split_once(';')?;
            Some((path.into(), secs.parse().ok()?))
        }),
        quit_after_capture: false,
        menus: menus::Menus::new(languages),
        special: Default::default(),
        assets,
        options: opts,
        options_path,
        last_info,
        unfocused_at: None,
        exit_requested: false,
        solo: None,
        menutest: (0, 0, None),
        weather: weather::WeatherState::with_images(weather_images),
        exact: Default::default(),
        riding: riding::Riding::new(),
        misc: Default::default(),
        front,
        debug: debug_screen::DebugScreen::new(Some(servers::game_dir())),
        tick_acc: 0.0,
        pause: Default::default(),
        sync: Default::default(),
        player: Player::new([0.0; 3], 0.0, 0.0),
        positioned: false,
        prev_pos: [0.0; 3],
        phys,
        light,
        relight: Vec::new(),
        relight_sky: Vec::new(),
        light_recv: light_engine::Received::default(),
        light_shadow: light_engine::Shadow::on().then(Box::default),
        player_id: 0,
        attr_names,
        last_input: None,
        free_cam: false,
        autowalk: rc_data::env_str!("RUSTCRAFT_AUTOWALK").and_then(|v| v.parse().ok()),
        autowalk_ticks: 0,
        trace: rc_data::env_os!("RUSTCRAFT_TRACE")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(std::io::BufWriter::new),
        camera: camera::Camera::default(),
        shot: camera::Shot { pos: [0.0; 3], yaw: 0.0, pitch: 0.0 },
        quick_play: args.launch.quick_play_singleplayer.clone(),
        quick_log: quickplay::Log::new(args.launch.quick_play_path.as_deref(), &servers::game_dir()),
    };
    // Quick play multiplayer: part of the log = address and name of its entry in the list
    // (otherwise the server's default name, translated on entering the game).
    if let Some(addr) = args.launch.quick_play_multiplayer.as_deref() {
        let list = servers::ServerList::load(&servers::game_dir());
        let name = list.find(addr).map(|e| e.name.clone());
        app.quick_log.set_world(quickplay::Kind::Multiplayer, addr, name.as_deref());
    }
    // `RUSTCRAFT_SOLO=<saves folder>`: singleplayer world opened at startup (test scripts).
    if let Some(w) = rc_data::env_str!("RUSTCRAFT_SOLO") {
        app.front.direct = true;
        app.open_world(&servers::game_dir().join("saves").join(w), false);
        boot::mark("singleplayer world opened");
    }
    present_watch::start();
    let el = EventLoop::new().expect("event loop");
    boot::mark("event loop");
    el.set_control_flow(ControlFlow::Poll);
    el.run_app(&mut app).expect("run");
    app.close_solo();
    rc_assets::cache::wait_writer();
}

/// Aiming shape depending on the item held in the main hand (`held`): full light block
/// only with a light in hand (empty otherwise: neither aimed nor broken), full scaffolding with
/// scaffolding in hand; otherwise the extracted outline shape (`outline`).
fn held_shape(block: &str, held: &str, outline: u16, full: Option<u16>) -> u16 {
    match full {
        Some(f) if held == block && matches!(block, "minecraft:light" | "minecraft:scaffolding") => f,
        _ => outline,
    }
}

/// Aiming and outline shape of a state at `pos` for the local player: shape index depending on
/// the held item (`held`) and offset of the blocks whose shape follows the model (`crack::shape_shift`).
fn aim_shape(
    data: &rc_data::VersionData,
    full: Option<u16>,
    held: &str,
    state: u32,
    pos: [i32; 3],
) -> Option<(u16, [f64; 3])> {
    let info = data.state(state)?;
    let block = &data.blocks.get(usize::from(info.block))?.name;
    Some((held_shape(block, held, info.outline, full), crack::shape_shift(data, state, pos)))
}

/// Local player drawn: detached view, or lying (the camera is then on them even in first
/// person); never in free camera nor when the camera follows another entity.
fn draws_local_player(detached: bool, sleeping: bool, free_cam: bool, other_camera: bool) -> bool {
    (detached || sleeping) && !free_cam && !other_camera
}

/// Column grid of a joined world (connection, respawn, dimension change):
/// server view radius, at least 2.
fn level_grid(server_view: i32) -> ChunkGrid {
    ChunkGrid::new(server_view.max(2) as u32)
}

/// The loaded world as seen by the physics.
struct GridWorld<'a> {
    grid: &'a ChunkGrid,
    min_y: i32,
    height: i32,
    t: &'a rc_physics::Tables,
    border: &'a rc_physics::WorldBorder,
    /// Solid entities for the one who moves (`entity_motion::colliders`).
    colliders: &'a [rc_physics::Aabb],
    /// Moving piston blocks (shape depending on progress), and direction of the push in
    /// progress in which they do not block.
    movers: Option<&'a block_entities::BlockEntities>,
    noclip: Option<u8>,
}

impl GridWorld<'_> {
    /// Shape of a moving piston block at (x, y, z); false if it is not one.
    fn piston_shape(&self, s: u32, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        if !self.t.is_moving_piston(s) {
            return false;
        }
        if let Some(m) = self.movers.and_then(|b| b.mover([x, y, z])) {
            self.t.moving_collision(&m, self.noclip, out);
        }
        true
    }
}

impl rc_physics::World for GridWorld<'_> {
    fn entity_colliders(&self, area: &rc_physics::Aabb, out: &mut Vec<rc_physics::Aabb>) {
        rc_physics::entity_boxes_in(self.colliders, area, out);
    }

    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        match (self.movers, self.state(x, y, z)) {
            (Some(b), Some(s)) if b.moving > 0 => self.piston_shape(s, x, y, z, out),
            _ => false,
        }
    }

    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let c = self.grid.get(x >> 4, z >> 4)?;
        let ly = y - self.min_y;
        if !(0..self.height).contains(&ly) {
            return Some(0);
        }
        c.block((x & 15) as usize, ly as usize, (z & 15) as usize)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        match self.state(x, y, z) {
            Some(s) => (self.movers.is_some_and(|b| b.moving > 0) && self.piston_shape(s, x, y, z, out)) || self.t.collision(s, out),
            None => false,
        }
    }

    fn border(&self) -> Option<&rc_physics::WorldBorder> {
        Some(self.border)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }
}

/// Name tags (`entities.md` part 2 § 1.8), from the farthest to the nearest.
#[allow(clippy::too_many_arguments)]
fn name_tags(
    r: &mut Renderer,
    ents: &entities::Entities,
    players: &players::PlayerList,
    world: &World,
    eye: [f64; 3],
    (yaw, pitch): (f32, f32),
    pt: f32,
    hidden: bool,
    target: Option<i32>,
    (scores, local_name, spectator): (&scoreboard::Scoreboard, &str, bool),
    ids: &stack::ComponentIds,
) {
    r.tags.clear();
    r.tags.sync(scores.version);
    let Some(er) = &r.entities else { return };
    if rc_data::env_flag!("RUSTCRAFT_NOENT") {
        return;
    }
    let mut list: Vec<(f64, i32, &entities::Entity)> = ents
        .map
        .iter()
        .filter_map(|(&id, e)| {
            let p = e.render_pos(pt);
            let d2: f64 = (0..3).map(|i| (p[i] - eye[i]).powi(2)).sum();
            let frame_named = || frame_item_name(e, ids).is_some();
            // Team rules (`scoreboard-teams.md` § 8): final result if the entity has a team.
            let team = |invisible: bool| {
                let holder = holder_name(e, players, !scores.members.is_empty());
                let (name, t) = holder.as_deref().and_then(|h| scores.team_named(h))?;
                let mine = scores.team_named(local_name).map(|(n, _)| n);
                let allied = mine == Some(name);
                let sees = spectator || (allied && t.params.options & 2 != 0);
                let visible = !invisible || sees;
                Some(match t.params.nametag {
                    1 => false,
                    2 if mine.is_some() => allied && (t.params.options & 2 != 0 || !invisible),
                    3 if mine.is_some() => !allied && !invisible,
                    _ => visible,
                })
            };
            let ctx = TagCtx { picked: target == Some(id), hud_hidden: hidden, spectator };
            tag_shown(e, d2, ctx, frame_named, team).then_some((d2, id, e))
        })
        .collect();
    if list.is_empty() {
        return;
    }
    list.sort_by(|a, b| b.0.total_cmp(&a.0));
    let view = environ::WorldView { grid: &world.grid, min_y: world.min_y, height: world.height };
    let origin = [eye[0].floor() as i32, eye[2].floor() as i32];
    let (right, up, _) = camera::axes(yaw, pitch);
    let q = r.queue_handle();
    r.tags.begin(right, up);
    let dims: Vec<[f32; 3]> = list.iter().map(|(_, _, e)| e.dims(&er.dims)).collect();
    let frame_names: Vec<Option<rc_proto::nbt::Nbt>> = list.iter().map(|(_, _, e)| frame_item_name(e, ids)).collect();
    for (((_, id, e), d), frame_name) in list.iter().zip(dims).zip(&frame_names) {
        use nametag::NameRef;
        let source = match e.meta.get(&2) {
            _ if let Some(n) = frame_name => NameRef::Nbt(n),
            _ if e.kind == "player" => NameRef::Text(players.name(e.uuid).unwrap_or_default()),
            Some(rc_proto_v777::entity::Meta::Text(Some(n))) => NameRef::Nbt(n),
            _ => NameRef::Type(&e.kind),
        };
        let chars = |lang: &rc_text::Lang| {
            let name = match source {
                NameRef::Text(s) => rc_text::Component::text(s),
                NameRef::Nbt(n) => rc_text::from_nbt(n).unwrap_or_else(|| rc_text::Component::text("")),
                NameRef::Type(k) => rc_text::Component::translate(format!("entity.minecraft.{k}"), vec![]),
            };
            // Formatting by the holder's team (`scoreboard-teams.md` § 4).
            let name = match holder_name(e, players, !scores.members.is_empty()) {
                Some(h) => scores.decorate(&h, name),
                None => name,
            };
            rc_text::styled_chars(&rc_text::flatten(&name, lang))
        };
        let p = e.render_pos(pt);
        let mut anchor = glam::Vec3::new(
            (p[0] - f64::from(origin[0])) as f32,
            (p[1] + f64::from(d[2]) + 0.5 + entity_render::crouch_offset(e)) as f32,
            (p[2] - f64::from(origin[1])) as f32,
        );
        // "deadmau5" player (ears): the two lines raised by 10 px of the text.
        if e.kind == "player" && players.name(e.uuid) == Some("deadmau5") {
            anchor.y += EARS_TAG_RAISE;
        }
        // Light at the entity's probe point (eyes).
        let light = view.light_pair(p[0].floor() as i32, (p[1] + f64::from(d[1])).floor() as i32, p[2].floor() as i32);
        let discrete = e.flags() & 0x02 != 0;
        // "Below name" line (`scoreboard-teams.md` § 7) under the attribute distance (10 by
        // default, fixed for non-living), name raised above.
        let dist2 = list.iter().find(|(_, i, _)| i == id).map_or(f64::MAX, |x| x.0);
        // Mannequin: its description replaces the scoreboard line.
        let below = || match e.kind.as_str() {
            "mannequin" => mannequin_description(e),
            _ if scores.slots[2].is_some() => holder_name(e, players, true).and_then(|h| scores.below_name(&h)),
            _ => None,
        };
        if dist2 < e.below_dist * e.below_dist
            && let Some(line) = below()
        {
            // Cache key of the text: an object counts there by its font (sprite, profile), not by
            // its carrier character, common to all the objects.
            let key = rc_text::flatten(&line, &r.gui.lang)
                .iter()
                .map(|s| match s.style.font.as_deref() {
                    Some(f) if rc_text::object_font(f).is_some() => f,
                    _ => s.text.as_str(),
                })
                .collect::<String>();
            let line_chars = |lang: &rc_text::Lang| rc_text::styled_chars(&rc_text::flatten(&line, lang));
            r.tags.push(&mut r.gui, &q, *id ^ 0x4000_0000, NameRef::Text(&key), line_chars, anchor, discrete, light);
            anchor.y += 9.0 * 1.15 * 0.025;
        }
        r.tags.push(&mut r.gui, &q, *id, source, chars, anchor, discrete, light);
    }
}

/// Line below a mannequin's name: its description (value 19), the default translated label
/// as long as the server has not sent one, nothing if hidden.
fn mannequin_description(e: &entities::Entity) -> Option<rc_text::Component> {
    match e.meta.get(&19) {
        Some(rc_proto_v777::entity::Meta::Text(Some(n))) => rc_text::from_nbt(n),
        Some(rc_proto_v777::entity::Meta::Text(None)) => None,
        _ => Some(rc_text::Component::translate("entity.minecraft.mannequin.label", vec![])),
    }
}

/// Offset of the name of a player with ears ("deadmau5"): 10 px of the text at scale 0.025.
const EARS_TAG_RAISE: f32 = 10.0 * 0.025;

/// Image context of the tags: targeted entity, hidden interface (F1), local spectator.
#[derive(Clone, Copy)]
struct TagCtx {
    picked: bool,
    hud_hidden: bool,
    spectator: bool,
}

/// Name tag retained at `d2` (square of the distance to the camera), rules of the kind:
/// - armor stand: only "always visible name", at the distance of its attribute (the
///   invisible holograms stay displayed, even with the interface hidden);
/// - frame: named item, aimed frame, interface visible;
/// - non-living: always visible name, or aimed custom name, at 64 blocks;
/// - living: attribute distance, crouching within 32, team rules (`team`, depending on
///   invisibility) otherwise visible, interface visible and without a passenger; then player, or the same
///   name conditions as the non-living.
fn tag_shown(
    e: &entities::Entity,
    d2: f64,
    c: TagCtx,
    frame_named: impl FnOnce() -> bool,
    team: impl FnOnce(bool) -> Option<bool>,
) -> bool {
    let named = matches!(e.meta.get(&2), Some(rc_proto_v777::entity::Meta::Text(Some(_))));
    let by_name = e.meta_bool(3) || (named && c.picked);
    match e.kind.as_str() {
        "armor_stand" => d2 < e.tag_dist * e.tag_dist && e.meta_bool(3),
        "item_frame" | "glow_item_frame" => d2 < 64.0 * 64.0 && !c.hud_hidden && c.picked && frame_named(),
        _ if e.class == entities::Kind::Other => d2 < 64.0 * 64.0 && by_name,
        _ => {
            if d2 >= e.tag_dist * e.tag_dist || (e.flags() & 0x02 != 0 && d2 >= 1024.0) {
                return false;
            }
            let invisible = e.flags() & 0x20 != 0;
            let living = match team(invisible) {
                Some(v) => v,
                None => !c.hud_hidden && (!invisible || c.spectator) && e.passengers.is_empty(),
            };
            living && (e.kind == "player" || by_name)
        }
    }
}

/// Custom name of a frame's item (the game then displays the item's name, not the frame's).
fn frame_item_name(e: &entities::Entity, ids: &stack::ComponentIds) -> Option<rc_proto::nbt::Nbt> {
    if !matches!(e.kind.as_str(), "item_frame" | "glow_item_frame") {
        return None;
    }
    match e.meta.get(&9) {
        Some(rc_proto_v777::entity::Meta::Item(Some(s))) if s.count > 0 => ids.custom_name_nbt(s),
        _ => None,
    }
}

/// Score holder of an entity: profile name of a player, UUID as text otherwise; `None` without
/// teams (nothing to format, no allocation).
/// Glowing entities (flag 6: Glowing effect, spectral arrow…): identity → outline colour
/// (that of their team, otherwise white).
fn glow_colors(
    ents: &entities::Entities,
    players: &players::PlayerList,
    scores: &scoreboard::Scoreboard,
) -> hashbrown::HashMap<u128, u32> {
    let teams = !scores.members.is_empty();
    ents.map
        .values()
        .chain(ents.me.as_ref())
        .filter(|e| e.flags() & 0x40 != 0)
        .map(|e| {
            let rgb = holder_name(e, players, teams).and_then(|h| scores.team_rgb(&h)).unwrap_or(0xff_ffff);
            (e.uuid, rgb)
        })
        .collect()
}

/// Invisible entities that the local player sees as a ghost: all of them in spectator, otherwise those of
/// their team if it sees its invisible allies (option 0x02).
fn ghost_set(
    ents: &entities::Entities,
    players: &players::PlayerList,
    scores: &scoreboard::Scoreboard,
    spectator: bool,
) -> hashbrown::HashSet<u128> {
    let invisible = ents.map.values().chain(ents.me.as_ref()).filter(|e| e.flags() & 0x20 != 0);
    if spectator {
        return invisible.map(|e| e.uuid).collect();
    }
    let teams = !scores.members.is_empty();
    let team_of = |e: &entities::Entity| holder_name(e, players, teams).and_then(|h| scores.members.get(&h).cloned());
    let Some(mine) = ents.me.as_ref().and_then(team_of) else { return Default::default() };
    if scores.teams.get(&mine).is_none_or(|t| t.params.options & 2 == 0) {
        return Default::default();
    }
    invisible.filter(|e| team_of(e).as_ref() == Some(&mine)).map(|e| e.uuid).collect()
}

/// Avatars in spectator mode: mode 3 in the player list, and the local player according to their
/// own mode.
fn spectator_set(
    ents: &entities::Entities,
    players: &players::PlayerList,
    local_spectator: bool,
) -> hashbrown::HashSet<u128> {
    let mut set: hashbrown::HashSet<u128> =
        players.map.iter().filter(|(_, p)| p.game_mode == 3).map(|(&u, _)| u).collect();
    if let Some(me) = ents.me.as_ref() {
        if local_spectator {
            set.insert(me.uuid);
        } else {
            set.remove(&me.uuid);
        }
    }
    set
}

fn holder_name(e: &entities::Entity, players: &players::PlayerList, teams: bool) -> Option<String> {
    if !teams {
        return None;
    }
    if e.kind == "player" {
        return players.name(e.uuid).map(str::to_owned);
    }
    let h = format!("{:032x}", e.uuid);
    Some(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

#[cfg(test)]
mod tests {
    use super::resolve_teleport;
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
}

#[cfg(test)]
mod fidelity_tests {
    use super::*;
    use crate::debug_render::tests::test_entity;
    use rc_proto_v777::entity::Meta;

    fn col(x: i32, z: i32) -> Column {
        Column {
            x,
            z,
            sections: Box::new([]),
            sky_light: Box::new([]),
            block_light: Box::new([]),
            heightmaps: Vec::new(),
        }
    }

    /// Respawn with a server view of 32: the grid keeps columns 0 and 35 side by side
    /// (a grid of radius 16, 35 cells wide, put them in the same cell). Covers the function
    /// alone: its call in the respawn is not tested (no harness for `App`).
    #[test]
    fn level_grid_follows_server_view() {
        let mut g = level_grid(32);
        g.insert(col(0, 0));
        g.insert(col(35, 0));
        assert!(g.get(0, 0).is_some() && g.get(35, 0).is_some());
        assert_eq!(g.len(), 2);
    }

    /// Mannequin: default label, description sent, nothing if hidden.
    #[test]
    fn mannequin_shows_its_description_below_the_name() {
        let mut e = test_entity("mannequin");
        let label = rc_text::Component::translate("entity.minecraft.mannequin.label", vec![]);
        assert_eq!(mannequin_description(&e), Some(label));
        e.meta.insert(19, Meta::Text(Some(rc_proto::nbt::Nbt::String("Vendor".into()))));
        assert_eq!(mannequin_description(&e), Some(rc_text::Component::text("Vendor")));
        e.meta.insert(19, Meta::Text(None));
        assert_eq!(mannequin_description(&e), None);
    }

    const CTX: TagCtx = TagCtx { picked: false, hud_hidden: false, spectator: false };

    fn shown(e: &entities::Entity, d2: f64, c: TagCtx) -> bool {
        tag_shown(e, d2, c, || false, |_| None)
    }

    /// Hologram: invisible armor stand with always-visible name → displayed (even with the interface
    /// hidden); without "visible name", never, even aimed and named.
    #[test]
    fn invisible_armor_stand_hologram_keeps_its_name() {
        let mut e = test_entity("armor_stand");
        e.meta.insert(0, Meta::Byte(0x20));
        e.meta.insert(2, Meta::Text(Some(rc_proto::nbt::Nbt::String("Welcome".into()))));
        e.meta.insert(3, Meta::Bool(true));
        assert!(shown(&e, 100.0, CTX));
        assert!(shown(&e, 100.0, TagCtx { hud_hidden: true, ..CTX }));
        // Team that would hide the name: no effect on the armor stand.
        assert!(tag_shown(&e, 100.0, CTX, || false, |_| Some(false)));
        e.meta.insert(3, Meta::Bool(false));
        assert!(!shown(&e, 100.0, TagCtx { picked: true, ..CTX }));
    }

    /// Tag distances of living entities: attribute `name_tag_distance` (64 by default); the non-
    /// living stay at 64.
    #[test]
    fn name_tag_distance_comes_from_attribute() {
        let mut zombie = test_entity("zombie");
        zombie.meta.insert(3, Meta::Bool(true));
        assert!(shown(&zombie, 63.9 * 63.9, CTX) && !shown(&zombie, 64.0 * 64.0, CTX));
        zombie.tag_dist = 100.0;
        assert!(shown(&zombie, 90.0 * 90.0, CTX));
        zombie.tag_dist = 8.0;
        assert!(!shown(&zombie, 9.0 * 9.0, CTX));
        // Invisible living entity without a team: hidden (except spectator); hidden interface: hidden.
        zombie.tag_dist = 64.0;
        zombie.meta.insert(0, Meta::Byte(0x20));
        assert!(!shown(&zombie, 4.0, CTX) && shown(&zombie, 4.0, TagCtx { spectator: true, ..CTX }));
        let mut cart = test_entity("minecart");
        cart.meta.insert(3, Meta::Bool(true));
        cart.tag_dist = 8.0;
        assert!(shown(&cart, 60.0 * 60.0, CTX));
    }

    #[test]
    fn sleeping_local_player_is_drawn_in_first_person() {
        assert!(draws_local_player(false, true, false, false));
        assert!(!draws_local_player(false, false, false, false));
        assert!(draws_local_player(true, false, false, false));
        assert!(!draws_local_player(true, true, true, false) && !draws_local_player(true, true, false, true));
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

    /// Light block aimable only with a light in hand; full scaffolding with scaffolding in hand.
    #[test]
    fn held_item_changes_aim_shape() {
        let full = Some(1);
        assert_eq!(held_shape("minecraft:light", "minecraft:light", 0, full), 1);
        assert_eq!(held_shape("minecraft:light", "minecraft:stone", 0, full), 0);
        assert_eq!(held_shape("minecraft:scaffolding", "minecraft:scaffolding", 707, full), 1);
        assert_eq!(held_shape("minecraft:scaffolding", "", 707, full), 707);
        assert_eq!(held_shape("minecraft:stone", "minecraft:stone", 1, full), 1);
    }
}
