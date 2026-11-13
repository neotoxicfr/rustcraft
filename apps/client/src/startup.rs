//! Program startup: command line, thread pool, resources, session, construction of `App` and the event loop.

use hashbrown::{HashMap, HashSet};
use rc_client::{Config, Session};
use rc_physics::Player;
use rc_world::ChunkGrid;
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::{ControlFlow, EventLoop};

use crate::{
    App, World, alloc_trace, audio_slot, block_anim, block_entities, boot, camera, chat, commands, crash_report,
    debug_screen, debug_sys, entities, environ, flow, front, gpu_backend, hud, hud_extra, interact, inventory, launch,
    level_events, light_engine, memstats, menus, options, pack_stack, particles, players, pool_threads, present_watch,
    quickplay, recipe_book, reload, resources, riding, scoreboard, screens, servers, skins, stack, tablist,
    tech_blocks, video, weather, world_sounds,
};

/// `[opt]` lines of the allocator settings applied at the start of `run`.
fn log_allocator_settings(
    commit: debug_sys::CommitOnDemand,
    purge_delay: Option<i64>,
    direct_calls: bool,
    single_flag: bool,
) {
    use debug_sys::CommitOnDemand as C;
    match commit {
        C::Applied(on) => eprintln!("[opt] allocator pages committed on demand: {on}"),
        C::UserVariable => eprintln!("[opt] allocator pages: user's MIMALLOC_PAGE_COMMIT_ON_DEMAND"),
        C::NotInLibrary => eprintln!("[opt] allocator pages: no on-demand commit in this mimalloc (v2)"),
    }
    eprintln!("[opt] allocator: direct calls {direct_calls}, single flag {single_flag}, long purge delay {purge_delay:?}");
    #[cfg(all(windows, feature = "mimalloc-win-direct-tls"))]
    match debug_sys::free_tls_slot_probe() {
        Some(slot) if slot < 64 => eprintln!("[opt] mimalloc direct TLS: lowest free slot {slot} (< 64)"),
        slot => eprintln!("[opt] mimalloc direct TLS: lowest free slot {slot:?}, NOT below 64: build without the feature"),
    }
}

/// Runs the game: arguments, startup steps, the event loop, then the shutdown.
pub(crate) fn run() {
    // Before any large allocation: allocator pages committed on demand.
    let commit = debug_sys::commit_on_demand();
    let purge_delay = debug_sys::long_purge_delay();
    let (direct_calls, single_flag) = debug_sys::read_allocator_switches();
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
    log_allocator_settings(commit, purge_delay, direct_calls, single_flag);
    if let Some(a) = args.launch.account() {
        rc_client::auth::set_account(a);
    }
    // Read-only account services started at startup, like the game (`session.rs`).
    rc_client::session::configure(rc_client::session::Config {
        proxy: args.launch.proxy.clone(),
        game_dir: Some(servers::game_dir()),
        disable_multiplayer: args.launch.disable_multiplayer,
        disable_chat: args.launch.disable_chat,
        offline_developer_mode: args.launch.offline_developer_mode,
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
    // Generation batch lines dated like the client's `[perf] [t s]` lines (bench window).
    rc_sim::generation_timing::set_log_origin(started);
    boot::start();
    boot::mark("entering main");
    // Driver, adapter and device opened while the resources load (`gpu_backend/early_open.rs`).
    let early_gpu = gpu_backend::EarlyGpuOpen::start();
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
    // GPU formats fixed from the mesh tables before any meshing task can read them.
    crate::render::init_gpu_formats(&tables);
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
        below_line_keys: Default::default(),
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
        early_gpu,
        held_event: None,
        meshes_tx,
        meshes_rx,
        mesh_pending: Vec::new(),
        in_flight: 0,
        all_held: None,
        mesh_candidates: Default::default(),
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
        window_title: crate::window_title::WindowTitle::new(),
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
        world_icon: Default::default(),
        cracked_model_faces: Vec::new(),
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
