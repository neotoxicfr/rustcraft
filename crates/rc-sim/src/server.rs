//! Integrated server (`integrated-server.md`): world opening, player entry through the memory
//! link, 20-ticks-per-second loop on its own thread, chunk tracking, time.

use hashbrown::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rc_client::memory::Peer;
use rc_client::{ChatTag, EntityEvent, Event};
use rc_data::VersionData;
use rc_proto::nbt::Nbt;

/// Stack of the integrated server thread (reserved, committed on use): pack functions
/// nested over thousands of levels, like the game whose queue has no depth
/// limit (W11, `commands::exec::MAX_NESTING`).
pub const SOLO_STACK: usize = 1 << 30;
use rc_proto_v777::chunk::{BlockEntity, ChunkPacket, LightData};
use rc_proto_v777::config::{DimensionHeight, Login, SpawnInfo};
use rc_world::LightLayer;

use crate::Error;
use crate::chunk::Chunk;
use crate::input::Input;
use crate::level::Level;
use crate::lock::SessionLock;
use crate::nbtio::{self, int, list};
use crate::player::{MENU_SLOTS, PlayerData, Stack};
use crate::registries::Registries;
use crate::states::States;
use crate::world::{Dimension, Load};

pub const TICK: Duration = Duration::from_millis(50);

/// What the client gives to singleplayer.
#[derive(Debug, Clone)]
pub struct Options {
    pub world: PathBuf,
    pub name: String,
    /// Profile UUID; replaced by the world's `singleplayer_uuid` if it exists.
    pub uuid: u128,
    pub info: rc_proto_v777::play::ClientInformation,
    /// Client simulation distance (`simulationDistance` setting), at least 2.
    pub sim_distance: i32,
}

/// Integrated server thread, seen from the client.
pub struct Handle {
    thread: Option<JoinHandle<()>>,
    paused: Arc<AtomicBool>,
    saving: Arc<AtomicBool>,
    /// S9: LAN opening request, read by the server.
    publish: Arc<std::sync::Mutex<Option<crate::lan::Publish>>>,
    /// Simulation distance requested by the client (setting changed in game).
    sim: Arc<AtomicI32>,
    /// World preparation (loading screen: progress, chunk map).
    prep: Arc<crate::prep::Prep>,
}

impl Handle {
    /// World preparation read by the loading screen.
    pub fn prep(&self) -> Arc<crate::prep::Prep> {
        self.prep.clone()
    }

    /// Simulation distance of the client's setting (the integrated server follows it every tick).
    pub fn set_simulation_distance(&self, d: i32) {
        self.sim.store(d.max(2), Ordering::Release);
    }

    /// Save in progress (snapshot taken, writes not yet finished): indicator of
    /// automatic save.
    pub fn saving(&self) -> bool {
        self.saving.load(Ordering::Acquire)
    }

    /// Singleplayer pause (pause screen open), `integrated-server.md` § 9.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    /// S9: opening to the local network (world options); result in the host's chat.
    pub fn publish(&self, req: crate::lan::Publish) {
        if let Ok(mut g) = self.publish.lock() {
            *g = Some(req);
        }
    }

    /// Waits for the server to finish (closing save included); to call after having
    /// closed the link (`Link::disconnect`).
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Opens the world on a dedicated thread and returns the client's session.
pub fn start(opts: Options, data: Arc<VersionData>) -> Result<(rc_client::Session, Handle), rc_client::Error> {
    let level_uuid = Level::read(&opts.world).ok().and_then(|l| l.singleplayer_uuid);
    let uuid = level_uuid.unwrap_or(opts.uuid);
    let (session, peer) = rc_client::memory::session(data.clone(), uuid)?;
    let paused = Arc::new(AtomicBool::new(false));
    let p = paused.clone();
    let saving = Arc::new(AtomicBool::new(false));
    let sv = saving.clone();
    let publish = Arc::new(std::sync::Mutex::new(None));
    let pb = publish.clone();
    let sim = Arc::new(AtomicI32::new(opts.sim_distance.max(2)));
    let sm = sim.clone();
    let prep = Arc::new(crate::prep::Prep::default());
    let pr = prep.clone();
    let thread = std::thread::Builder::new().name("singleplayer".into()).stack_size(SOLO_STACK).spawn(move || {
        let _ = peer.send(Event::LoginFinished);
        match Server::open(opts, uuid, data, peer, p) {
            Ok(mut s) => {
                s.saving = sv;
                s.publish_req = pb; // S9
                s.sim_req = sm; // W9: tracked simulation distance
                s.prep_open(pr);
                s.run()
            }
            Err((peer, e)) => {
                eprintln!("singleplayer: {e}");
                let _ = peer.send(Event::Kicked(rc_text::Component::text(e.to_string())));
                let _ = peer.send(Event::Disconnected(e.to_string()));
            }
        }
    })?;
    Ok((session, Handle { thread: Some(thread), paused, saving, publish, sim, prep }))
}

/// Slot of a chunk on the server side: loaded, or absent / not loadable (nothing sent).
type Slot = Option<Box<Chunk>>;

pub struct Server {
    pub(crate) data: Arc<VersionData>,
    pub(crate) peer: Peer,
    pub(crate) world: PathBuf,
    /// World generation settings, read once at opening (`dims.rs`).
    pub(crate) wgs: crate::dims::GenSettings,
    lock: Option<SessionLock>,
    pub(crate) level: Level,
    /// Shared with the chunk read thread (`loader.rs`).
    pub(crate) states: Arc<States>,
    pub(crate) regs: Registries,
    pub(crate) dim: Dimension,
    pub(crate) chunks: HashMap<(i32, i32), Slot>,
    /// Frozen entities (raw NBT) per chunk, with their entity id.
    pub(crate) entities: HashMap<(i32, i32), Vec<(i32, Nbt)>>,
    pub(crate) next_entity: i32,
    pub(crate) paused: Arc<AtomicBool>,
    /// Save in progress (read by the client: indicator), reset to false by the write thread.
    pub(crate) saving: Arc<AtomicBool>,
    /// Simulation distance (client setting) and its shared request.
    pub(crate) sim_dist: i32,
    pub(crate) sim_req: Arc<AtomicI32>,
    was_paused: bool,
    pub(crate) tick: u64,
    // Player.
    pub(crate) player_id: i32,
    pub(crate) uuid: u128,
    pub(crate) name: String,
    /// Host profile properties (signed skin of the launcher's account), like the game's
    /// singleplayer profile.
    pub(crate) host_properties: (u128, Vec<rc_proto_v777::players::Property>),
    pub(crate) pdata: PlayerData,
    pub(crate) pos: [f64; 3],
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) on_ground: bool,
    pub(crate) game_mode: i32,
    pub(crate) flying: bool,
    pub(crate) held: i32,
    pub(crate) slots: Vec<Option<Stack>>,
    pub(crate) loaded: bool,
    pub(crate) joined_tick: u64,
    /// Mining ticks lived by this player (the game's own counter): advances on every mining
    /// tick, restarts from zero only with a new player (connection, respawn).
    pub(crate) mine_ticks: i64,
    /// World opened in demo mode (launch option `--demo`): all players are in it.
    pub(crate) demo_world: bool,
    /// Demo state of the current player (swapped with the guests).
    pub(crate) demo: crate::demo::Demo,
    /// Pending teleport: number, expected position, send tick.
    pub(crate) teleport: Option<(i32, [f64; 3], u64)>,
    pub(crate) teleport_id: i32,
    pub(crate) moved_this_tick: bool,
    // Tracked chunks.
    pub(crate) view: i32,
    pub(crate) center: Option<(i32, i32)>,
    pub(crate) sent: HashSet<(i32, i32)>,
    pub(crate) queue: Vec<(i32, i32)>,
    /// Read thread of the send queue (`RUSTCRAFT_OPT_ASYNC_LOAD`), created on first use.
    pub(crate) loader: Option<crate::loader::Loader>,
    // Time.
    pub(crate) time: i64,
    pub(crate) clocks: Vec<(String, i64)>,
    pub(crate) rules: Nbt,
    pub(crate) ack: Option<i32>,
    /// Blocks changed during the tick (broadcast at the end of the tick).
    pub(crate) changed: Vec<[i32; 3]>,
    /// Ticks remaining before the next automatic save.
    pub(crate) autosave: u64,
    /// Feature flags of the world (union of those of the chosen packs), registry order.
    pub(crate) features: Vec<String>,
    /// Collisions (movement replay) and light.
    pub(crate) phys: rc_physics::Tables,
    pub(crate) light: rc_light::LightTables,
    /// Positions whose light must be recomputed at the end of the tick.
    pub(crate) relight: Vec<[i32; 3]>,
    /// Positions of `relight` that touch the sky (attenuation or shape), picked up at write time.
    pub(crate) relight_sky: Vec<[i32; 3]>,
    /// Relight switches (`play::LightOpts`).
    pub(crate) light_opts: crate::play::LightOpts,
    /// Block tags (tool rules), sorted ids.
    pub(crate) block_tags: HashMap<String, Vec<u32>>,
    /// The same tags as bits (`RUSTCRAFT_OPT_TAG_BITS`), rebuilt with `block_tags`.
    pub(crate) tag_bits: crate::tag_bits::TagBits,
    /// Mob rules per state (walkability), `RUSTCRAFT_OPT_MOBSTATES`.
    pub(crate) mob_states: crate::mobs::env::MobStates,
    /// Last sky darkening and its key (`RUSTCRAFT_OPT_SKYDARK`).
    pub(crate) sky_darken: crate::mobs::env::SkyDarkenCache,
    pub(crate) mining: Option<crate::play::Mining>,
    /// Displayed skin parts and main arm (`client_information`, metadata 16 and 15).
    pub(crate) model: (u8, i32),
    /// Last accepted position (§ 4.2).
    pub(crate) last_good: [f64; 3],
    pub(crate) sneaking: bool,
    /// Click on a block that started from inside the block (field of the `use_item_on` packet), read by
    /// scaffolding placement.
    pub(crate) hit_inside: bool,
    /// Clocks as read (written only if they advanced).
    pub(crate) clocks_loaded: Vec<(String, i64)>,
    /// Save write thread (created at the first one).
    pub(crate) writer: Option<crate::save::Writer>,
    /// Block behaviors: tick queues, notifications, world randomness.
    pub(crate) bs: crate::blocks::level::Sim,
    /// Survival loop (milestone 2, `life.rs`).
    pub(crate) life: Box<crate::life::Life>,
    /// Mobs (S4, `mobs/`).
    pub(crate) mobs: Box<crate::mobs::Mobs>,
    /// Simulated decorations (paintings, frames, armor stands, cushions, `decoration/`).
    pub(crate) deco: Box<crate::decoration::Decos>,
    /// Generation of missing chunks (`generation.rs`).
    pub(crate) generator: Option<crate::generation::Gen>,
    /// Other dimensions and portals (S6, `dims.rs`).
    pub(crate) dims: Box<crate::dims::Dims>,
    /// Sends cut off (tick of a parked dimension).
    pub(crate) muted: bool,
    /// Commands (S5, `commands/`).
    pub(crate) cmd: Box<crate::commands::Cmds>,
    /// Projectiles of the open world and shooting player's state (S8, `projectiles/`).
    pub(crate) proj: Box<crate::projectiles::Projs>,
    pub(crate) proj_player: Box<crate::projectiles::PlayerProj>,
    /// Minecarts of the open world and the player's minecart (G5-F06, `minecart/`).
    pub(crate) carts: Box<crate::minecart::Carts>,
    pub(crate) pcart: crate::minecart::PlayerCart,
    /// World maps (S8, `maps.rs`).
    pub(crate) maps: Box<crate::maps::Maps>,
    /// Advancements and statistics of the player (S9, `progress/`, `stats.rs`).
    pub(crate) prog: Box<crate::progress::Progress>,
    /// Local network (S9, `lan/`): open world, opening request.
    pub(crate) lan: Option<Box<crate::lan::Lan>>,
    pub(crate) publish_req: Arc<std::sync::Mutex<Option<crate::lan::Publish>>>,
    /// Tracked preparation for the loading screen, until the player enters (`prep.rs`).
    pub(crate) prep: Option<Box<crate::prep::Track>>,
}

/// Dimensions of the world, in registration order.
pub const DIMENSIONS: [&str; 3] = ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];

#[cfg(test)]
thread_local! {
    /// Benches: cumulative time of the world's block and fluid pass (`tick_level_blocks`, without the block entities).
    pub(crate) static BLOCKS_NS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(crate) fn sha256_seed(seed: i64) -> i64 {
    let d = ring::digest::digest(&ring::digest::SHA256, &seed.to_le_bytes());
    i64::from_le_bytes(d.as_ref()[..8].try_into().unwrap_or_default())
}

impl Server {
    pub(crate) fn open(
        opts: Options,
        uuid: u128,
        data: Arc<VersionData>,
        peer: Peer,
        paused: Arc<AtomicBool>,
    ) -> Result<Server, (Peer, Error)> {
        let world = opts.world.clone();
        let fail = |peer, e| Err((peer, e));
        if crate::mem_stats::enabled() {
            crate::mem_stats::reset(); // previous world's readout (same process) cleared
        }
        // Startup readouts, per step ("in game" column of the scene bench).
        let opened = Instant::now();
        let mut lap = opened;
        let mut step = |what: &str| {
            eprintln!("singleplayer: {what} in {:.0} ms", lap.elapsed().as_secs_f64() * 1e3);
            lap = Instant::now();
        };
        let lock = match SessionLock::take(&world) {
            Ok(l) => l,
            Err(e) => return fail(peer, e),
        };
        let mut level = match Level::read(&world) {
            Ok(l) => l,
            Err(e) => return fail(peer, e),
        };
        if level.data_version != crate::chunk::DATA_VERSION {
            let e = Error::Refused(format!(
                "world with data version {}: open it first with the official game 26.3",
                level.data_version
            ));
            return fail(peer, e);
        }
        let Some(jar) = crate::registries::client_jar(&data) else {
            return fail(peer, Error::Refused("game jar not found (registries)".into()));
        };
        let mut jar = match crate::registries::Jar::read(&jar) {
            Ok(j) => j,
            Err(e) => return fail(peer, e),
        };
        // World data packs chosen according to level.dat and stacked on the jar; the
        // computed selection is rewritten into level.dat, like the game.
        let (builtin_packs, packs) = crate::packs::open(&mut jar, &world, level.root.get("Data"));
        if let Some(d) = nbtio::get_mut(&mut level.root, "Data") {
            crate::packs::write_config(d, &packs);
        }
        step("lock, level.dat, jar and packs read");
        let regs = crate::registries::load_jar(&jar, &data);
        let block_tags = block_tag_map(&regs.tags);
        let tag_bits = crate::tag_bits::TagBits::new(&block_tags, &data);
        let mut phys = rc_physics::Tables::new(&data);
        if let Some((_, t)) = regs.tags.iter().find(|(r, _)| r == "minecraft:block") {
            phys.set_block_tags(t);
        }
        let light = rc_light::LightTables::new(&data);
        let states = States::new(&data, regs.entries("minecraft:worldgen/biome").to_vec())
            .with_heightmap_rules(&data, &block_tags);
        step("registries and tables");
        let pdata = PlayerData::read(&world, uuid);
        let dim_name = pdata
            .dimension
            .clone()
            .filter(|d| DIMENSIONS.contains(&d.as_str()))
            .unwrap_or_else(|| level.spawn_dimension.clone());
        let dim = Dimension::new(&world, &dim_name, crate::dimension_height(&data, &dim_name));
        let read = |f: &str| nbtio::read_dat(&world.join("data").join("minecraft").join(f)).ok();
        let rules = read("game_rules.dat").and_then(|v| v.get("data").cloned()).unwrap_or(Nbt::Compound(Vec::new()));
        let clocks = match read("world_clocks.dat").and_then(|v| v.get("data").cloned()) {
            Some(Nbt::Compound(f)) => f.iter().map(|(k, v)| (k.clone(), int(v, "total_ticks").unwrap_or(0))).collect(),
            _ => vec![("minecraft:overworld".to_owned(), 0)],
        };
        let spawn = level.spawn_pos;
        let pos =
            pdata.pos.unwrap_or_else(|| [f64::from(spawn[0]) + 0.5, f64::from(spawn[1]), f64::from(spawn[2]) + 0.5]);
        let [yaw, pitch] = pdata.rotation.unwrap_or([level.spawn_yaw, level.spawn_pitch]);
        let game_mode = pdata.game_mode.unwrap_or(level.game_type);
        let life = Box::new(crate::life::Life::new(&jar, &data, &regs, &world, &pdata.root));
        // Language of the texts rendered server-side (en_us, read from the jar once per process):
        // loaded at opening rather than at the first command, which otherwise froze a tick
        // (up to 1.5 s measured under load, the other servers waiting for the same load).
        crate::commands::cmds_ents::SERVER_LANG.get_or_init(|| crate::stations::en_us(&life.items));
        step("player, survival and language");
        let generator = crate::generation::start_for(&data, &world, &dim_name, &regs);
        step("generator started");
        let mut cmd = Box::new(crate::commands::Cmds::new(
            &jar,
            &data,
            &regs,
            &world,
            &dim.dir,
            &opts.name,
            level.allow_commands,
        )); // S5
        cmd.builtin_packs = builtin_packs;
        let tree = crate::progress::Tree::shared(&jar, packs.vanilla_only()); // S9
        let prog = Box::new(crate::progress::Progress::new(tree, &world, uuid)); // S9
        step("commands and advancements");
        eprintln!("singleplayer: opened in {:.0} ms", opened.elapsed().as_secs_f64() * 1e3);
        // Before the literal: uses no random source (the order of test seeds does not move).
        let mob_states = crate::mobs::env::MobStates::new(&data, &crate::blocks::table::fluid_mob(&data));
        Ok(Server {
            prog,
            prep: None,
            lan: None,
            publish_req: Arc::new(std::sync::Mutex::new(None)),
            cmd,
            life,
            mobs: Box::default(),
            deco: Box::default(),
            proj: Box::default(),        // S8
            proj_player: Box::default(), // S8
            carts: Box::default(),       // G5-F06
            pcart: crate::minecart::PlayerCart::default(),
            maps: Box::default(), // S8
            peer,
            wgs: crate::dims::GenSettings::read(&world),
            world,
            lock: Some(lock),
            time: level.time,
            states: Arc::new(states),
            regs,
            dim,
            chunks: HashMap::new(),
            entities: HashMap::new(),
            next_entity: 2,
            paused,
            saving: Arc::new(AtomicBool::new(false)),
            sim_dist: opts.sim_distance.max(2),
            sim_req: Arc::new(AtomicI32::new(opts.sim_distance.max(2))),
            was_paused: false,
            tick: 0,
            player_id: 1,
            uuid,
            host_properties: (uuid, host_properties(&opts.name)),
            name: opts.name,
            pos,
            yaw,
            pitch,
            on_ground: false,
            game_mode,
            // Mode abilities on load (like the game): spectator always flying,
            // creative keeps its state, survival and adventure on the ground.
            flying: match game_mode {
                3 => true,
                1 => pdata.flying,
                _ => false,
            },
            held: pdata.selected,
            slots: pdata.slots.clone(),
            loaded: false,
            joined_tick: 0,
            mine_ticks: 0,
            demo_world: crate::demo::launch_demo(),
            demo: crate::demo::Demo::default(),
            teleport: None,
            teleport_id: 0,
            moved_this_tick: false,
            view: i32::from(opts.info.view_distance).max(2),
            center: None,
            sent: HashSet::new(),
            queue: Vec::new(),
            loader: None,
            clocks: clocks.clone(),
            rules,
            ack: None,
            changed: Vec::new(),
            autosave: autosave_first(),
            features: packs.features,
            phys,
            light,
            relight: Vec::new(),
            relight_sky: Vec::new(),
            light_opts: crate::play::LightOpts::current(),
            block_tags,
            tag_bits,
            mob_states,
            sky_darken: Default::default(),
            mining: None,
            model: (opts.info.skin_parts, opts.info.main_hand),
            last_good: pos,
            sneaking: false,
            hit_inside: false,
            clocks_loaded: clocks.clone(),
            writer: None,
            bs: crate::blocks::level::Sim::new(&data),
            generator,
            dims: Box::default(),
            muted: false,
            pdata,
            level,
            data,
        })
    }

    pub(crate) fn rule(&self, name: &str, default: bool) -> bool {
        self.rules.get(name).and_then(Nbt::as_i64).map_or(default, |v| v != 0)
    }

    pub(crate) fn send(&self, e: Event) -> bool {
        if self.muted {
            return false;
        }
        if self.lan.is_some() {
            return self.lan_route(e); // S9: LAN host and guests
        }
        self.peer.send(e)
    }

    fn dimension_type_index(&self) -> i32 {
        let t = self.dimension_type_of(&self.dim.name); // S6
        self.regs.entries("minecraft:dimension_type").iter().position(|n| *n == t).unwrap_or(0) as i32
    }

    pub(crate) fn flat(&self) -> bool {
        self.wgs.is_flat(&self.dim.name)
    }

    pub(crate) fn seed(&self) -> i64 {
        self.wgs.seed()
    }

    /// Abilities by mode (creative: flight and instant break; spectator: flight).
    pub(crate) fn abilities(&self) -> rc_proto_v777::play::Abilities {
        let creative = self.game_mode == 1;
        let spectator = self.game_mode == 3;
        rc_proto_v777::play::Abilities {
            invulnerable: creative || spectator,
            flying: self.flying && (creative || spectator),
            may_fly: creative || spectator,
            instabuild: creative,
            flying_speed: self.pdata.fly_speed,
            walking_speed: self.pdata.walk_speed,
        }
    }

    /// Entering the game (§ 2.1 to 2.4): registries, tags, `login` and world information.
    pub(crate) fn join(&mut self) {
        self.dims_read_player(); // S6: portal delay, credits seen
        let _ = self.send(Event::EnabledFeatures(self.features.clone()));
        for (name, entries) in &self.regs.synced {
            if name.ends_with("_variant") || name == "minecraft:enchantment" || name == "minecraft:dialog" {
                let entries = entries.iter().map(|e| (e.clone(), None)).collect();
                let _ = self.peer.send(Event::Registry { name: name.clone(), entries });
            }
        }
        let _ = self.send(Event::Tags(self.regs.tags.clone()));
        let flat = self.flat();
        let height = DimensionHeight {
            min_y: self.dim.height.min_y,
            height: self.dim.height.sections as i32 * 16,
            nether_light: self.data.dimension_types.iter().any(|t| t.name == self.dim.name && t.nether_light),
        };
        let login = Login {
            player_id: self.player_id,
            hardcore: self.level.hardcore,
            dimensions: self.world_dimensions(), // S6
            max_players: 8,
            view_distance: self.lan_server_view(), // the server's (the host's), guests included
            simulation_distance: self.sim_dist,
            reduced_debug_info: self.rule("minecraft:reduced_debug_info", false),
            show_death_screen: !self.rule("minecraft:immediate_respawn", false),
            limited_crafting: self.rule("minecraft:limited_crafting", false),
            spawn: SpawnInfo {
                dimension_type: self.dimension_type_index(),
                dimension: self.dim.name.clone(),
                hashed_seed: sha256_seed(self.seed()),
                game_mode: self.game_mode,
                previous_game_mode: self.pdata.previous_game_mode,
                debug: false,
                flat,
                last_death: self.last_death_pos(),
                portal_cooldown: self.dims.player.cooldown, // S6
                sea_level: if flat { -63 } else { 63 },
            },
            online_mode: false,
            enforces_secure_chat: false,
        };
        let _ = self.send(Event::Joined {
            login: Box::new(login),
            height,
            biomes: self.states.biome_names.clone(),
            dimension: self.dim.name.clone(),
            flat,
        });
        let _ = self.send(Event::Difficulty { id: self.level.difficulty_id(), locked: self.level.difficulty_locked });
        let _ = self.send(Event::Abilities(self.abilities()));
        let _ = self.send(Event::HeldSlot(self.held));
        // Current player's level: host (4 if the world allows commands), guest (`guest_level`).
        let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: crate::lan::op_event(self.cmd.op) }));
        self.send_commands(); // S5: command tree of the player's level
        self.send_scoreboard(); // S5: displayed teams and objectives
        // Game display name: team, entity hover, insertion, "/tell" click.
        let mut joined = rc_text::Component::translate(
            "multiplayer.player.joined",
            vec![rc_text::Arg::Component(self.player_display_name())],
        );
        joined.style.color = Some(0xFFFF55);
        let _ = self.send(Event::Chat { component: joined, tag: ChatTag::System });
        self.teleport_to(self.pos);
        use rc_proto_v777::players::{InfoEntry, InfoUpdate, action};
        let _ = self.send(Event::PlayerInfo(InfoUpdate {
            actions: action::ADD_PLAYER
                | action::UPDATE_GAME_MODE
                | action::UPDATE_LISTED
                | action::UPDATE_LATENCY
                | action::UPDATE_HAT,
            entries: vec![InfoEntry {
                uuid: self.uuid,
                profile: Some((self.name.clone(), self.profile_properties(self.uuid))),
                game_mode: Some(self.game_mode),
                listed: Some(true),
                latency: Some(0),
                show_hat: Some(self.model.0 & crate::play::HAT != 0),
                ..Default::default()
            }],
        }));
        // Skin parts and main arm other than default values (none, right).
        let mut values = Vec::new();
        if self.model.1 != 1 {
            values.push((15, rc_proto_v777::entity::Meta::Variant(self.model.1, 42)));
        }
        if self.model.0 != 0 {
            values.push((16, rc_proto_v777::entity::Meta::Byte(self.model.0 as i8)));
        }
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id: self.player_id, values }));
        }
        self.send_border();
        self.send_time(true);
        let _ = self
            .send(Event::SpawnPosition { dimension: self.level.spawn_dimension.clone(), pos: self.level.spawn_pos });
        let _ = self.send(Event::GameEvent { kind: 13, param: 0.0 });
        self.send_ticking_state(); // current tick rate (`tick`)
        self.send_ticking_step();
        self.update_tracking();
        self.send_recipe_sets(); // S2: furnace items
        self.send_recipe_book(); // S2: recipe book
        self.send_weather_state(); // S2: rain in progress
        self.send_inventory();
        self.send_health(); // S2: health, hunger, experience
        self.send_attributes();
        self.send_all_effects(); // S7
        self.progress_join(); // S9: advancements (first packet), statistics to resend
        self.load_root_vehicle(); // F04: remounts the saved vehicle
        self.joined_tick = self.tick;
    }

    /// Player attributes changed since the last send (W8a, `attrs.rs`): file bases,
    /// `/attribute`, creative mode, worn equipment, effects; only the transmissible attributes.
    pub(crate) fn send_attributes(&mut self) {
        self.sync_attributes();
    }

    pub(crate) fn send_border(&self) {
        let b = nbtio::read_dat(&self.dim.dir.join("data").join("minecraft").join("world_border.dat"))
            .ok()
            .and_then(|v| v.get("data").cloned());
        let f = |k: &str, d: f64| match b.as_ref().and_then(|b| b.get(k)) {
            Some(Nbt::Double(v)) => *v,
            Some(o) => o.as_i64().map_or(d, |i| i as f64),
            None => d,
        };
        let size = f("size", 59_999_968.0);
        let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::Init {
            x: f("center_x", 0.0),
            z: f("center_z", 0.0),
            old_size: size,
            new_size: f("lerp_target", size),
            ticks: f("lerp_time", 0.0) as i64,
            max_size: 29_999_984,
            warn_blocks: f("warning_blocks", 5.0) as i32,
            warn_ticks: f("warning_time", 15.0) as i32,
        }));
    }

    /// `set_time`: full (clocks) or game time alone.
    pub(crate) fn send_time(&self, full: bool) {
        let rate = if self.rule("minecraft:advance_time", true) { 1.0 } else { 0.0 };
        let clocks = if full {
            self.clocks.iter().map(|(n, t)| (n.clone(), *t, 0.0, self.clock_net_rate(n, rate))).collect()
        } else {
            Vec::new()
        };
        let _ = self.send(Event::Time { game_time: self.time, clocks });
    }

    pub(crate) fn send_inventory(&mut self) {
        self.life_send_inventory(); // S2: full stacks, tracked state number
    }

    /// Absolute teleport (§ 4.1): next number, expected position.
    pub(crate) fn teleport_to(&mut self, pos: [f64; 3]) {
        self.teleport_id = if self.teleport_id == i32::MAX { 0 } else { self.teleport_id + 1 };
        self.pos = pos;
        self.teleport = Some((self.teleport_id, pos, self.tick));
        self.send_teleport();
    }

    pub(crate) fn send_teleport(&self) {
        let Some((id, pos, _)) = self.teleport else { return };
        let _ = self.send(Event::Teleport(rc_proto_v777::play::PlayerPosition {
            id,
            pos,
            delta: [0.0; 3],
            yaw: self.yaw,
            pitch: self.pitch,
            relative: 0,
        }));
    }

    pub(crate) fn chunk_pos(&self) -> (i32, i32) {
        ((self.pos[0].floor() as i32) >> 4, (self.pos[2].floor() as i32) >> 4)
    }

    fn tracked(&self, (x, z): (i32, i32), (cx, cz): (i32, i32)) -> bool {
        let dx = ((x - cx).abs() - 1).max(0);
        let dz = ((z - cz).abs() - 1).max(0);
        dx * dx + dz * dz < self.view * self.view
    }

    /// Tracked area (§ 3.1): center, incoming queued, outgoing forgotten.
    pub(crate) fn update_tracking(&mut self) {
        let c = self.chunk_pos();
        if self.center == Some(c) {
            return;
        }
        let _ = self.send(Event::ChunkCenter { x: c.0, z: c.1 });
        self.center = Some(c);
        let r = self.view + 1;
        // Queue as a table: with sliced sending it stays long between two ticks (linear search
        // by position otherwise quadratic on a center change).
        let queued: HashSet<(i32, i32)> = self.queue.iter().copied().collect();
        for x in c.0 - r..=c.0 + r {
            for z in c.1 - r..=c.1 + r {
                if self.tracked((x, z), c) && !self.sent.contains(&(x, z)) && !queued.contains(&(x, z)) {
                    self.queue.push((x, z));
                }
            }
        }
        let mut q = std::mem::take(&mut self.queue);
        q.retain(|&p| self.tracked(p, c));
        self.queue = q;
        // Reads in progress of a chunk that left the view: thrown away on arrival.
        #[cfg(test)]
        let keep_all = crate::loader::sabotage() == 5;
        #[cfg(not(test))]
        let keep_all = false;
        if let Some(mut l) = self.loader.take() {
            l.pending.retain(|&p| keep_all || self.tracked(p, c));
            self.loader = Some(l);
        }
        let gone: Vec<(i32, i32)> = self.sent.iter().copied().filter(|&p| !self.tracked(p, c)).collect();
        for (x, z) in gone {
            self.sent.remove(&(x, z));
            let _ = self.send(Event::ForgetChunk { x, z });
            // Entities removed from the view, kept for the save (never simulated).
            if let Some(ents) = self.entities.get(&(x, z))
                && !ents.is_empty()
            {
                let _ = self.send(Event::Entity(EntityEvent::Remove(ents.iter().map(|e| e.0).collect())));
            }
        }
    }

    /// Loads the requested chunks (in parallel) if they are not.
    pub(crate) fn ensure_loaded(&mut self, wanted: &[(i32, i32)]) {
        self.ensure_loaded_split(wanted, usize::MAX);
    }

    /// [`Server::ensure_loaded`], reads in tasks of at most `per_task` chunks per region.
    fn ensure_loaded_split(&mut self, wanted: &[(i32, i32)], per_task: usize) {
        let missing: Vec<(i32, i32)> = wanted.iter().copied().filter(|p| !self.chunks.contains_key(p)).collect();
        if missing.is_empty() {
            return;
        }
        let read = crate::loader::Read::of(&self.dim, &missing, &self.data, &self.states, per_task);
        self.adopt_read(&missing, read);
    }

    /// Placement of chunks read (`missing`: positions in the order of the request): synchronous
    /// path and read thread batches go through here.
    pub(crate) fn adopt_read(&mut self, missing: &[(i32, i32)], read: crate::loader::Read) {
        #[cfg(test)]
        crate::loader::ADOPTED.with(|a| a.borrow_mut().extend_from_slice(missing));
        let mut generate = Vec::new();
        for (p, l) in read.chunks {
            let slot = match l {
                Load::Loaded(c) => Some(c),
                Load::Absent | Load::Skipped(crate::chunk::Skip::NotFull(_)) => {
                    generate.push(p);
                    None
                }
                Load::Skipped(s) => {
                    if !matches!(s, crate::chunk::Skip::NotFull(_)) {
                        eprintln!("singleplayer: chunk {p:?} ignored: {s}");
                    }
                    None
                }
                Load::Unreadable(e) => {
                    eprintln!("singleplayer: chunk {p:?} unreadable: {e}");
                    None
                }
            };
            self.chunks.insert(p, slot);
            self.import_chunk_ticks(p);
        }
        if let Some(g) = &mut self.generator {
            g.want(generate);
        }
        for (p, nbt) in read.entities {
            let list: Vec<(i32, Nbt)> = nbt
                .iter()
                .flat_map(|n| list(n.get("Entities")).iter().cloned())
                .map(|e| {
                    self.next_entity += 1;
                    (self.next_entity, e)
                })
                .collect();
            self.entities.insert(p, list);
        }
        self.poi_loaded(read.poi); // F05: points of interest
        self.discover_furnaces(missing); // S2: simulated furnaces
        self.adopt_mobs(missing); // S4: mobs
        self.adopt_projectiles(missing); // S8: arrows, tridents, clouds
        self.adopt_carts(missing); // G5-F06: minecarts
        self.deco_adopt(missing); // paintings, frames, stands, cushions
    }

    /// Batches of the read thread (`RUSTCRAFT_OPT_ASYNC_LOAD`), picked up at the start of the tick and adopted
    /// in the order of the requests. A position loaded in the meantime (synchronous read by another
    /// caller) or no longer expected (left the view, other dimension) is thrown away.
    pub(crate) fn poll_loader(&mut self) {
        let Some(l) = &mut self.loader else { return };
        #[allow(unused_mut)]
        let mut batches = l.ready();
        #[cfg(test)]
        if crate::loader::sabotage() == 3 {
            // Sabotage: batches and positions adopted in reverse of the request.
            batches.reverse();
            batches.iter_mut().for_each(|b| b.list.reverse());
        }
        for mut b in batches {
            let Some(l) = &mut self.loader else { return };
            if b.dim != self.dim.name || l.dim != b.dim {
                continue;
            }
            #[cfg(test)]
            let skip_filter = crate::loader::sabotage() == 4;
            #[cfg(not(test))]
            let skip_filter = false;
            let keep: Vec<(i32, i32)> = b
                .list
                .iter()
                .copied()
                .filter(|p| l.pending.remove(p) && (skip_filter || !self.chunks.contains_key(p)))
                .collect();
            if keep.len() != b.list.len() {
                let set: HashSet<(i32, i32)> = keep.iter().copied().collect();
                b.read.retain(&set);
            }
            if !keep.is_empty() {
                self.adopt_read(&keep, b.read);
            }
        }
    }

    /// Sending of the pending chunks (§ 3.2): memory connection, the whole queue every tick,
    /// by distance to the player. The batch is acknowledged right away, like the client's network thread
    /// does on receiving `chunk_batch_finished`.
    pub(crate) fn send_chunks(&mut self) {
        // S9: LAN guest: batches capped by the rate they acknowledge.
        if let Some(i) = self.lan.as_ref().and_then(|l| l.active) {
            self.lan_send_chunks(i);
            return;
        }
        if self.queue.is_empty() {
            return;
        }
        let (cx, cz) = self.chunk_pos();
        let mut q = std::mem::take(&mut self.queue);
        q.sort_by_key(|&(x, z)| (x - cx) * (x - cx) + (z - cz) * (z - cz));
        // Spectator, `spectators_generate_chunks` rule false: it neither loads nor generates anything
        // (neither sliced reads nor on the thread); only already loaded chunks
        // reach it, the others wait in the queue.
        let loads = self.loads_chunks();
        let (stream, read_apart) = (crate::loader::stream_on(), crate::loader::async_on());
        if !loads || (!stream && !read_apart) {
            if loads {
                self.ensure_loaded(&q);
            }
            self.send_loaded(&q);
            return;
        }
        // Slices: the first goes up to the last chunk of the spawn square (queue sorted
        // by distance), the following ones of `SLICE` as long as the tick's budget allows; without
        // sliced sending, a single slice (the whole queue).
        let r = crate::loader::SPAWN_RADIUS;
        let first = if stream {
            q.iter().rposition(|&(x, z)| (x - cx).abs() <= r && (z - cz).abs() <= r).map_or(0, |i| i + 1)
        } else {
            q.len()
        };
        let started = Instant::now();
        let mut done = 0;
        while done < q.len() {
            let end = if done == 0 && first > 0 { first } else { (done + crate::loader::SLICE).min(q.len()) };
            let slice = &q[done..end];
            // Spawn square read right away, the rest on the thread (`RUSTCRAFT_OPT_ASYNC_NEARSYNC`).
            let near_now = read_apart && stream && done == 0 && first > 0 && crate::loader::near_sync();
            if read_apart && !near_now {
                let missing: Vec<(i32, i32)> = slice.iter().copied().filter(|p| !self.chunks.contains_key(p)).collect();
                if self.loader.is_none() && !missing.is_empty() {
                    self.loader = crate::loader::Loader::start(self.data.clone(), self.states.clone());
                }
                match &mut self.loader {
                    Some(l) => l.want(&self.dim, &missing),
                    // Thread impossible to start: synchronous read.
                    None => self.ensure_loaded_split(slice, crate::loader::per_task()),
                }
            } else {
                self.ensure_loaded_split(slice, crate::loader::per_task());
            }
            self.send_loaded(slice);
            done = end;
            if stream && started.elapsed() >= crate::loader::budget() {
                break;
            }
        }
        // Separate read: the rest is requested this very tick, even with the budget exhausted (in slices,
        // in queue order).
        if read_apart && done < q.len() && self.loader.is_none() {
            self.loader = crate::loader::Loader::start(self.data.clone(), self.states.clone());
        }
        if read_apart && let Some(l) = &mut self.loader {
            for slice in q[done..].chunks(crate::loader::SLICE) {
                let missing: Vec<(i32, i32)> = slice.iter().copied().filter(|p| !self.chunks.contains_key(p)).collect();
                l.want(&self.dim, &missing);
            }
        }
        // Rest of the queue: for the next tick (neither loaded nor marked sent).
        self.queue.extend_from_slice(&q[done..]);
    }

    /// The current player loads and generates chunks (false for a spectator under the
    /// `spectators_generate_chunks` rule false).
    fn loads_chunks(&self) -> bool {
        self.game_mode != 3 || self.rule("minecraft:spectators_generate_chunks", true)
    }

    /// Sends the loaded chunks of `list`; those being generated or read stay in the queue.
    fn send_loaded(&mut self, list: &[(i32, i32)]) {
        let loads = self.loads_chunks();
        for &p in list {
            // Being generated: stays in the queue until it arrives.
            if self.generator.as_ref().is_some_and(|g| g.pending.contains(&p))
                || (!loads && !self.chunks.get(&p).is_some_and(Option::is_some))
            {
                self.queue.push(p);
                continue;
            }
            // Being read on the separate thread: same.
            #[cfg(test)]
            let early = crate::loader::sabotage() == 1;
            #[cfg(not(test))]
            let early = false;
            if !self.chunks.contains_key(&p) && !early {
                self.queue.push(p);
                continue;
            }
            #[cfg(test)]
            if crate::loader::sabotage() == 2 && self.sent.is_empty() {
                // Sabotage: first chunk forgotten (neither sent nor put back in the queue).
                self.sent.insert((i32::MIN, i32::MIN));
                continue;
            }
            self.sent.insert(p);
            let Some(Some(c)) = self.chunks.get(&p) else { continue };
            let packet = self.chunk_packet(c);
            let _ = self.send(Event::Chunk(Box::new(packet)));
            self.show_entities(p);
        }
    }

    pub(crate) fn show_entities(&self, p: (i32, i32)) {
        let Some(list) = self.entities.get(&p) else { return };
        for (id, e) in list {
            // Spawn (painting and frames at the attachment block), metadata, equipment.
            self.deco_announce(*id, e);
        }
    }

    /// Chunk packet as the client decodes it (no encoding).
    pub(crate) fn chunk_packet(&self, c: &Chunk) -> ChunkPacket {
        let heightmaps = c.heightmaps.iter().filter(|(k, _)| matches!(k, 1 | 4 | 5)).cloned().collect();
        let be_types = self.data.registry("minecraft:block_entity_type");
        let block_entities = c
            .block_entities
            .iter()
            .filter_map(|b| {
                let kind =
                    be_types?.entries.iter().position(|t| Some(t.as_str()) == b.get("id").and_then(Nbt::as_str))?;
                let (x, y, z) = (int(b, "x")? as i32, int(b, "y")? as i16, int(b, "z")? as i32);
                let mut nbt = b.clone();
                for k in ["id", "x", "y", "z", "keepPacked"] {
                    nbtio::remove(&mut nbt, k);
                }
                Some(BlockEntity { x: (x & 15) as u8, z: (z & 15) as u8, y, kind: kind as i32, data: Some(nbt) })
            })
            .collect();
        ChunkPacket {
            x: c.x,
            z: c.z,
            heightmaps,
            sections: c.sections.clone(),
            block_entities,
            light: light_data(&c.sky, &c.block, self.light_opts.sections),
        }
    }

    pub(crate) fn handle(&mut self, input: Input) {
        match input {
            Input::AcceptTeleport { id } => {
                if let Some((n, pos, _)) = self.teleport
                    && n == id
                {
                    self.pos = pos;
                    self.last_good = pos;
                    self.teleport = None;
                }
            }
            Input::Move { pos, rot, on_ground } => {
                let (before, was) = (self.pos, self.on_ground);
                self.on_move(pos, rot, on_ground);
                self.after_move(before, was); // S2: fall, exhaustion
                self.progress_after_move(before, was); // S9: statistics, blocks crossed
                self.rec_player_moved(before); // F14: step vibrations, shrieker
                if pos.is_some() {
                    self.proj_known_move(before); // S8: known movement (shots)
                }
            }
            Input::TickEnd => {
                self.moved_this_tick = false;
                self.proj_tick_end(); // S8
            }
            Input::Loaded => self.loaded = true,
            Input::Keys(k) => {
                self.sneaking = k & 32 != 0;
                self.prog.keys = k; // S9: `input` predicate
            }
            Input::PickBlock { pos, with_data } => self.pick_block(pos, with_data),
            Input::ClientInfo { view, skin_parts, main_hand } => {
                self.set_model(skin_parts, main_hand);
                self.on_view_request(view); // S9: guests capped by the host's view
            }
            Input::SpectatorAction(target) => self.on_spectator_action(target),
            Input::HeldSlot(s) if (0..9).contains(&s) => {
                // Changing slot stops the main hand's use in progress (eating, drinking,
                // drawing the bow…) without finishing it; end vibration included.
                if i32::from(s) != self.held && self.life.vit.using.is_some_and(|(h, _)| h == 0) {
                    self.stop_using();
                }
                self.held = i32::from(s);
            }
            Input::Flying(f) => self.flying = f && self.abilities().may_fly,
            Input::CreativeSlot { slot, item } if self.game_mode == 1 && (1..MENU_SLOTS as i16).contains(&slot) => {
                let names = self.data.registry("minecraft:item").map(|r| &r.entries);
                self.slots[slot as usize] = item.and_then(|(id, count)| {
                    let id = names?.get(id as usize)?.clone();
                    Some(Stack { id, count, components: None })
                });
            }
            Input::Action { action, pos, face, seq } => self.on_action(action, pos, face, seq),
            Input::UseItemOn { hand, pos, face, at, inside, seq } => {
                self.on_use_item_on(hand, pos, face, at, inside, seq)
            }
            Input::UseItem { seq } => self.acknowledge(seq),
            _ => {}
        }
    }

    pub(crate) fn acknowledge(&mut self, seq: i32) {
        self.ack = Some(self.ack.map_or(seq, |a| a.max(seq)));
    }

    /// "Loaded" player (§ 2.6): `player_loaded` received or 60 ticks elapsed.
    pub(crate) fn player_ready(&self) -> bool {
        self.loaded || self.tick >= self.joined_tick + 60
    }

    /// One tick: clocks, world (deferred mining, broadcast of changed blocks), then
    /// "network" phase of the player (§ 4.3): the sequence acknowledgement therefore leaves after the `block_update`s.
    /// Frozen (`tick freeze`), only the player ticks: clocks, weather, blocks, entities, block entities
    /// and functions wait, except during the requested steps (`tick step`).
    pub(crate) fn game_tick(&mut self) {
        self.cmd.ticking.tick();
        let runs = self.cmd.ticking.runs();
        // Game order: server functions, clocks, then world (weather, sleep, game
        // time and scheduled functions, blocks, entities).
        self.functions_tick(); // S5: `#minecraft:load` after (re)load, `#minecraft:tick` (if the game is running)
        if runs {
            self.time += 1;
            if self.rule("minecraft:advance_time", true) {
                self.advance_clocks(); // S5: pause and speed of the clocks (`time`)
            }
        }
        if self.tick.is_multiple_of(20) {
            self.send_time(false);
        }
        if runs {
            self.border_tick(); // moving border, before the weather
            self.weather_tick(); // S2: weather
            self.sleep_world_tick(); // S2: night skip (all players)
            self.run_scheduled(); // S5: scheduled functions, before blocks and entities
            #[cfg(test)]
            let t = Instant::now();
            self.tick_level_blocks();
            #[cfg(test)]
            BLOCKS_NS.with(|n| n.set(n.get() + t.elapsed().as_nanos() as u64));
        }
        if let Some((_, _, at)) = self.teleport
            && self.tick > at + 20
        {
            self.teleport = self.teleport.map(|(n, p, _)| (n, p, self.tick));
            self.send_teleport();
        }
        self.tick_mining();
        self.demo_tick(); // demo mode: after mining, like the game
        self.life_tick(); // S2: furnaces, items, health, menus
        self.mobs_tick(); // S4: mobs
        self.deco_tick(); // decorations: survival of what is attached
        self.command_minecarts_tick(); // G5-F06: minecarts (command block included), once per tick
        self.projectiles_tick(); // S8: projectiles, clouds, fishing
        // Block entities after all the entities, like the game: a piston pushes an entity
        // after its own movement of the tick (slime momentum consumed on the next tick).
        if runs {
            self.tick_block_entities();
        }
        self.maps_tick(); // S8: held maps
        self.lodestone_compass_tick(); // G5-F10: lodestone compasses
        self.dims_tick(); // S6: portals, parked dimensions
        self.commands_tick(); // S5 : suggestions, scores
        self.progress_tick(); // S9: time statistics, triggers, sending of advancements
        self.flush_block_changes();
        if let Some(a) = self.ack.take() {
            let _ = self.send(Event::BlockChangedAck(a));
        }
    }

    fn run(&mut self) {
        self.join();
        let mut clock = crate::tickrate::Schedule::new(Instant::now());
        // Client tick graph (F3+2, always kept by the game's integrated server).
        let mut last_full = Instant::now();
        loop {
            // (0) duration of this tick: cadence of `tick rate`, zero in a sprint; big lag skipped
            // at most once per interval, otherwise caught up by chaining ticks.
            let sprinting = {
                let paused = self.paused.load(Ordering::Acquire) && self.lan.is_none();
                let mut report = None;
                let sprint = !paused && self.cmd.ticking.sprinting() && self.cmd.ticking.sprint_this_tick(&mut report);
                if let Some(r) = report {
                    self.sprint_finished(&r);
                }
                sprint
            };
            if sprinting {
                clock.sprint(Instant::now());
            } else {
                let tick = self.cmd.ticking.nanos_per_tick();
                let skipped = clock.plan(Instant::now(), tick);
                if skipped > 0 {
                    eprintln!("singleplayer: lagging by {skipped} ticks, skipped");
                }
            }
            let work = Instant::now();
            crate::commands::cmds_ents::debug_profiler_tick_start(self.tick); // `/debug start`
            // (0 bis) chunks read on the separate thread, adopted before everything else in the tick.
            self.poll_loader();
            // (1) client messages.
            loop {
                match self.peer.packets.try_recv() {
                    // S9, S2, S4, S5: own packets, then general inputs.
                    Ok(p) => self.dispatch_packet(&p),
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        self.close();
                        return;
                    }
                }
            }
            self.lan_requests(); // S9: opening requested, LAN guests
            // Simulation distance setting changed: tracked and announced (`set_simulation_distance`).
            let sd = self.sim_req.load(Ordering::Acquire).max(2);
            if sd != self.sim_dist {
                self.sim_dist = sd;
                let _ = self.send(Event::SimulationDistance(sd));
            }
            let method_start = Instant::now();
            // (2) tick, outside pause (§ 9); an open world does not pause.
            let paused = self.paused.load(Ordering::Acquire) && self.lan.is_none();
            if paused != self.was_paused {
                self.was_paused = paused;
                if paused {
                    self.save(false);
                } else {
                    self.send_time(false);
                }
            }
            if paused {
                self.paused_tick();
            } else {
                self.game_tick();
                if self.lan.is_some() {
                    self.lan_tick(); // S9: guests
                }
                self.tick += 1;
            }
            // (3) chunks.
            self.update_tracking();
            self.poll_gen();
            // Batches finished during the tick: adopted right away, sent in this tick rather
            // than the next (`RUSTCRAFT_OPT_ASYNC_LATE`).
            if crate::loader::late_poll() {
                self.poll_loader();
            }
            self.send_chunks();
            self.prep_tick(); // loading screen: progress and map
            // (4) automatic save (§ 8).
            if !paused {
                self.autosave = self.autosave.saturating_sub(1);
                if self.autosave == 0 {
                    self.autosave = self.autosave_interval();
                    self.save(false);
                }
            }
            self.cmd.ticking.record_tick_time(work.elapsed());
            perf_tick_report(&self.cmd.ticking);
            // Memory readout (`RUSTCRAFT_PERFLOG`, bounded batch) AFTER the tick measurement: counted
            // inside, it added ~0.3 ms to the bench's median tick (0.36 ms in pause) without costing
            // anything in normal play, where it does not run.
            if crate::mem_stats::enabled() {
                self.mem_walk();
            }
            let wait_start = Instant::now();
            let wait = clock.wait(wait_start);
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
            let end = Instant::now();
            let ns = |d: Duration| d.as_nanos() as i64;
            self.peer.stats().push_tick_sample([
                ns(end - last_full),
                ns(wait_start - method_start),
                ns(method_start - work),
                ns(end - wait_start),
            ]);
            last_full = end;
            if sprinting {
                self.cmd.ticking.end_tick_work();
            }
        }
    }

    /// Closing (§ 8): full save waiting for the writes, then lock released.
    fn close(&mut self) {
        self.lan_close(); // S9
        self.stop_gen();
        self.stop_parked(); // S6
        self.mount_unload_with_player(); // F04: the vehicle leaves with the player
        self.award_custom("leave_game", 1); // S9: leaving statistic (like removing a player from the list)
        self.save(true);
        if let Some(l) = self.lock.take() {
            l.release();
        }
    }

    /// Properties of the profile of a player of the game: the account's for the host, none
    /// for the others (LAN guests, without session verification).
    pub(crate) fn profile_properties(&self, uuid: u128) -> Vec<rc_proto_v777::players::Property> {
        // LAN guests: properties of their verified profile (`lan/auth.rs`).
        if uuid == self.host_properties.0 { self.host_properties.1.clone() } else { self.lan_properties(uuid) }
    }
}

/// Block tags sorted by name (binary search of the members).
fn block_tag_map(tags: &crate::registries::Tags) -> HashMap<String, Vec<u32>> {
    tags.iter()
        .find(|(r, _)| r == "minecraft:block")
        .map(|(_, t)| {
            t.iter()
                .map(|(n, ids)| {
                    let mut v: Vec<u32> = ids.iter().map(|&i| i as u32).collect();
                    v.sort_unstable();
                    (n.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Server {
    /// Tags reloaded (`/reload`, `/datapack`): kept, applied to the collisions and the block
    /// rules, sent back to the client like the game.
    pub(crate) fn set_registry_tags(&mut self, tags: crate::registries::Tags) {
        self.regs.tags = tags;
        self.block_tags = block_tag_map(&self.regs.tags);
        self.tag_bits = crate::tag_bits::TagBits::new(&self.block_tags, &self.data);
        if let Some((_, t)) = self.regs.tags.iter().find(|(r, _)| r == "minecraft:block") {
            self.phys.set_block_tags(t);
        }
        let _ = self.send(Event::Tags(self.regs.tags.clone()));
    }
}

/// Singleplayer profile, like the game: the account profile read by the services at startup, kept
/// if the requested nickname is its own (case-insensitive); it carries the player's signed skin.
fn host_properties(name: &str) -> Vec<rc_proto_v777::players::Property> {
    host_properties_of(rc_client::session::own_profile().as_ref(), name)
}

fn host_properties_of(
    profile: Option<&rc_client::session::ProfileResult>,
    name: &str,
) -> Vec<rc_proto_v777::players::Property> {
    profile.filter(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.properties.clone()).unwrap_or_default()
}

/// Stored light → packet light data, all the present layers. `empty_mask`
/// (`RUSTCRAFT_OPT_LIGHTSECTIONS`): a null layer goes into the empty mask, without its 2048
/// bytes, like the game for a never-written layer (see [`light_data_filtered`]).
pub(crate) fn light_data(sky: &[LightLayer], block: &[LightLayer], empty_mask: bool) -> LightData {
    if empty_mask {
        return light_data_with(sky, block, None, None);
    }
    light_data_all(sky, block)
}

/// Like the game: only the positions of the filters (one bit per layer position, sky and block
/// separate); absent layer: nothing; null layer: empty mask; otherwise mask and data.
/// Accepted difference: the game sends as data a layer already written that returned to 0 (torch removed);
/// the client receives the same zeros.
pub(crate) fn light_data_filtered(sky: &[LightLayer], block: &[LightLayer], sky_f: &[u64], block_f: &[u64]) -> LightData {
    light_data_with(sky, block, Some(sky_f), Some(block_f))
}

fn light_data_with(sky: &[LightLayer], block: &[LightLayer], sky_f: Option<&[u64]>, block_f: Option<&[u64]>) -> LightData {
    let zero = rc_world::zero_layer();
    let pick = |l: &[LightLayer], f: Option<&[u64]>| {
        let words = l.len().div_ceil(64);
        let (mut out, mut mask, mut empty) = (vec![None; l.len()], vec![0u64; words], vec![0u64; words]);
        for (i, x) in l.iter().enumerate() {
            if f.is_some_and(|f| f.get(i / 64).is_none_or(|w| w >> (i % 64) & 1 == 0)) {
                continue;
            }
            let Some(x) = x else { continue };
            // 128-bit words, exit at the first non-zero (micro-bench `bench_light_data`: overhead
            // +580 ns per column of 52 layers, half null, versus +860 for 256-byte blocks
            // reduced by OR and +800 for an OR of the whole byte by byte).
            let is_zero = std::sync::Arc::ptr_eq(x, &zero)
                || x.as_chunks::<16>().0.iter().all(|w| u128::from_ne_bytes(*w) == 0);
            if is_zero && !crate::play::light_sabotage_empty() {
                empty[i / 64] |= 1 << (i % 64);
            } else {
                mask[i / 64] |= 1 << (i % 64);
            }
            out[i] = Some(x.clone());
        }
        (out, mask, empty)
    };
    let (sky, sky_mask, sky_empty) = pick(sky, sky_f);
    let (block, block_mask, block_empty) = pick(block, block_f);
    LightData { sky, block, sky_empty, block_empty, sky_mask, block_mask }
}

/// Old path: all the present layers as data, empty mask never set.
fn light_data_all(sky: &[LightLayer], block: &[LightLayer]) -> LightData {
    let mask = |l: &[LightLayer]| {
        let mut m = vec![0u64; l.len().div_ceil(64)];
        for (i, x) in l.iter().enumerate() {
            if x.is_some() {
                m[i / 64] |= 1 << (i % 64);
            }
        }
        m
    };
    LightData {
        sky: sky.to_vec(),
        block: block.to_vec(),
        sky_empty: Vec::new(),
        block_empty: Vec::new(),
        sky_mask: mask(sky),
        block_mask: mask(block),
    }
}

/// Imposed interval of the automatic save: `RUSTCRAFT_AUTOSAVE_TICKS=n` (n > 0) shortens
/// it for the bench's `save` scene. Read once.
fn autosave_override() -> Option<u64> {
    static T: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *T.get_or_init(|| parse_autosave_ticks(rc_data::env_str!("RUSTCRAFT_AUTOSAVE_TICKS")))
}

fn parse_autosave_ticks(v: Option<&str>) -> Option<u64> {
    v.and_then(|s| s.trim().parse().ok()).filter(|&n| n > 0)
}

/// First automatic save: 6000 ticks after opening, like the game (no computation).
fn autosave_first() -> u64 {
    autosave_override().unwrap_or(6000)
}

/// Next interval, like the game: 300 s at the current rate (in a sprint, estimated rate
/// from the average tick duration), at least 100 ticks.
fn autosave_interval_for(t: &crate::tickrate::TickRate) -> u64 {
    let tps = if t.sprinting() { 1e9_f32 / (t.average_tick_nanos() + 1) as f32 } else { t.rate() };
    ((tps * 300.0) as i32).max(100) as u64
}

impl Server {
    fn autosave_interval(&self) -> u64 {
        autosave_override().unwrap_or_else(|| autosave_interval_for(&self.cmd.ticking))
    }

    /// Rate changed (`tick rate`, end of sprint): the deadline is only brought closer.
    pub(crate) fn on_tick_rate_changed(&mut self) {
        self.autosave = self.autosave.min(self.autosave_interval());
    }
}

#[cfg(test)]
mod autosave_tests {
    use crate::tickrate::TickRate;

    #[test]
    fn override_is_parsed_strictly() {
        assert_eq!(super::parse_autosave_ticks(None), None);
        assert_eq!(super::parse_autosave_ticks(Some("0")), None);
        assert_eq!(super::parse_autosave_ticks(Some("abc")), None);
        assert_eq!(super::parse_autosave_ticks(Some(" 200 ")), Some(200));
    }

    #[test]
    fn interval_follows_the_tick_rate_with_a_floor() {
        let mut t = TickRate::default();
        assert_eq!(super::autosave_interval_for(&t), 6000);
        t.set_rate(100.0);
        assert_eq!(super::autosave_interval_for(&t), 30000);
        t.set_rate(1.0);
        assert_eq!(super::autosave_interval_for(&t), 300);
        t.set_rate(0.2); // brought back to 1 tick/s by the rate
        assert_eq!(super::autosave_interval_for(&t), 300);
        // Sprint: mean tick of 1 ms (≈ 1000 ticks/s) → 300,000 ticks.
        t.set_rate(20.0);
        t.request_sprint(10);
        for _ in 0..100 {
            t.record_tick_time(std::time::Duration::from_millis(1));
        }
        assert_eq!(super::autosave_interval_for(&t), 299_999);
        // Floor at 100: impossible here without a rate < 1/3 tick/s, checked on the formula.
        assert_eq!(((0.1_f32 * 300.0) as i32).max(100), 100);
    }
}

/// Measurements (`RUSTCRAFT_PERFLOG`, `docs/perf.md`): every 100 ticks, median,
/// 95th and 99th percentiles and mean of the work of the last 100 ticks of the integrated server.
fn perf_tick_report(t: &crate::tickrate::TickRate) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if !*ON.get_or_init(|| rc_data::env_flag!("RUSTCRAFT_PERFLOG")) {
        return;
    }
    if !(N.fetch_add(1, Ordering::Relaxed) + 1).is_multiple_of(100) {
        return;
    }
    let (p50, p95, p99, _) = t.percentiles();
    let ms = |ns: u64| ns as f64 / 1e6;
    eprintln!(
        "[perf] server: tick median {:.3} ms, p95 {:.3} ms, p99 {:.3} ms, mean {:.3} ms",
        ms(p50),
        ms(p95),
        ms(p99),
        ms(t.average_tick_nanos())
    );
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn solo_profile_carries_the_account_skin_only_for_its_name() {
        let skin = rc_proto_v777::players::Property {
            name: "textures".into(),
            value: "e30=".into(),
            signature: Some("c2ln".into()),
        };
        let p = rc_client::session::ProfileResult {
            id: 7,
            name: "Steve".into(),
            properties: vec![skin.clone()],
            forced_name_change: false,
            banned_skin: false,
        };
        assert_eq!(host_properties_of(Some(&p), "steve"), vec![skin]);
        assert!(host_properties_of(Some(&p), "Alex").is_empty());
        assert!(host_properties_of(None, "Steve").is_empty());
    }
}
