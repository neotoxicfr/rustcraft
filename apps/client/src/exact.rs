//! Exactness packets on the game side (`docs/specs/protocol-supplements.md`): batches applied as a
//! block, and the events that `main.rs` does not handle itself.

use rc_client::Event;

use crate::App;

/// Clock in ms of the ping requests (that of `netstats`, so that the delay is right).
fn now_ms() -> i64 {
    rc_client::netstats::millis()
}

impl App {
    /// A session event: a batch is unrolled here, sub-packets in order, in the same
    /// frame and before the tick (§ 1).
    pub(crate) fn deliver(&mut self, ev: Event) {
        if let Event::Bundle(events) = ev {
            for e in events {
                self.deliver(e);
            }
            return;
        }
        let ev = match ev {
            // Registry dialogs (`show_dialog` by id).
            Event::Registry { name, entries } if name == "minecraft:dialog" => {
                self.exact.dialogs = entries;
                return;
            }
            // Transfer: nothing of the old session is applied any more (§ 10).
            Event::Transfer { host, port } => {
                self.transfer(host, port);
                return;
            }
            ev => ev,
        };
        // New world: default rate, the server sends back its own just after (§ 2).
        if matches!(ev, Event::Joined { .. } | Event::Respawn { .. }) {
            self.exact.rate = Default::default();
            // A new world puts the camera back on the player (§ 6).
            self.exact.camera = None;
        }
        if let Event::Joined { login, .. } = &ev {
            self.exact.simulation_distance = login.simulation_distance;
        }
        if let Some(ev) = self.session_event(ev) {
            self.apply(ev);
        }
    }

    /// Events not handled by `apply` (packets of this module).
    pub(crate) fn apply_exact(&mut self, ev: Event) {
        match ev {
            Event::TickingState { rate, frozen } => self.exact.rate.set_state(rate, frozen),
            Event::TickingStep(n) => self.exact.rate.steps = n,
            Event::Explode(x) => self.on_explode(*x),
            Event::PlayerRotation { yaw, yaw_relative, pitch, pitch_relative } => {
                self.on_player_rotation(yaw, yaw_relative, pitch, pitch_relative);
            }
            Event::LookAt { eyes, target, entity } => self.on_look_at(eyes, target, entity),
            Event::SetCamera(id) => self.on_set_camera(id),
            Event::MoveVehicle { pos, yaw, pitch } => self.on_move_vehicle(pos, yaw, pitch),
            Event::EnabledFeatures(f) => {
                for x in f.iter().filter(|x| !KNOWN_FEATURES.contains(&x.as_str())) {
                    eprintln!("unknown feature ignored: {x}");
                }
                self.exact.features = f.into_iter().filter(|x| KNOWN_FEATURES.contains(&x.as_str())).collect();
            }
            // Chat carried over from the previous session discarded (§ 11): messages only, settings kept.
            Event::ResetChat => self.chat.clear_session(),
            Event::ReportDetails(d) => self.exact.report = d,
            Event::ServerLinks(l) => self.exact.links = l,
            Event::ShowDialog(d) => {
                let config = self.session.link.names.phase() == rc_data::Phase::Configuration;
                self.show_dialog(d, config);
            }
            Event::ClearDialog => self.clear_dialog(),
            Event::ChunkBiomes(cols) => self.apply_biomes(cols),
            Event::SimulationDistance(d) => self.exact.simulation_distance = d,
            Event::Difficulty { id, locked } => self.exact.difficulty = (id, locked),
            // World options (`world_options.rs`) and code of conduct (`notices.rs`).
            Event::GameRuleValues(v) => self.menus.game_rules(v),
            Event::CodeOfConduct(text) => self.on_code_of_conduct(text),
            // Server resource packs (`server_packs.rs`).
            Event::PackPush { id, url, hash, required, prompt } => self.on_pack_push(id, url, hash, required, prompt),
            Event::PackPop(which) => self.on_pack_pop(which),
            _ => {}
        }
    }

    /// `chunks_biomes` (§ 13): biome containers replaced, neighboring columns re-meshed.
    fn apply_biomes(&mut self, cols: Vec<(i32, i32, Vec<rc_world::Container>)>) {
        for (x, z, biomes) in cols {
            // Same containers (`RUSTCRAFT_OPT_LIGHTNOCOPY`): neither column copy nor re-meshing
            // (identical tints).
            if crate::light_engine::opts().no_copy
                && self.world.grid.get(x, z).is_some_and(|c| c.sections.iter().zip(&biomes).all(|(s, b)| s.biomes == *b))
            {
                continue;
            }
            let Some(col) = self.world.grid.get_mut(x, z) else {
                eprintln!("biomes of an absent column ({x}, {z}) ignored");
                continue;
            };
            if rc_client::protolog::enabled() {
                eprintln!("biomes: column ({x}, {z}), {} sections", biomes.len());
            }
            for (s, b) in col.sections.iter_mut().zip(biomes) {
                s.biomes = b;
            }
            // Tints recomputed at meshing: sections of the 3 × 3 columns to rebuild.
            self.world.touch_with_neighbors(x, z);
            self.weather.rev += 1;
        }
    }

    /// `transfer` (§ 10): connection closed without disconnect screen, new connection to
    /// `host:port` with intent 3 and its cookies.
    fn transfer(&mut self, host: String, port: u16) {
        let cookies = self.session.link.cookies.clone();
        eprintln!("transfer to {host}:{port}");
        self.disconnect_quietly();
        let ip = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
        self.connect_to(crate::servers::ServerEntry::new(ip.clone(), ip), Some(cookies));
        self.front.set_status("connect.transferring");
    }

    /// Client tick, after that of the entities: ping test.
    pub(crate) fn exact_tick(&mut self) {
        self.dialog_tick();
        // Camera on a disappeared entity: nothing to follow (the server will send `set_camera` back).
        if self.exact.rate.runs {
            self.explosion_particles();
        }
        // `RUSTCRAFT_TICKLOG`: every real second, rate received, client ticks done and
        // position of the nearest non-player entity (visible freeze).
        if rc_data::env_flag!("RUSTCRAFT_TICKLOG") {
            let now = std::time::Instant::now();
            let since = *self.exact.log_since.get_or_insert(now);
            self.exact.log_ticks += 1;
            if now.duration_since(since).as_secs_f64() >= 1.0 {
                let r = self.exact.rate;
                let p = self.player.pos;
                let d = |e: &crate::entities::Entity| (0..3).map(|i| (e.pos[i] - p[i]).powi(2)).sum::<f64>();
                let near =
                    self.entities.map.values().filter(|e| e.kind != "player").min_by(|a, b| d(a).total_cmp(&d(b)));
                eprintln!(
                    "rate: {} t/s, freeze {}, step {}, advance {}, tick {:.0} ms, {} client ticks/s, {}",
                    r.rate,
                    r.frozen,
                    r.steps,
                    r.runs,
                    r.tick_len() * 1000.0,
                    self.exact.log_ticks,
                    near.map_or_else(
                        || "no entity".into(),
                        |e| format!("{} at {:.3} {:.3} {:.3}", e.kind, e.pos[0], e.pos[1], e.pos[2])
                    ),
                );
                self.exact.log_since = Some(now);
                self.exact.log_ticks = 0;
            }
        }
        // `RUSTCRAFT_PINGTEST`: one ping request per second, latencies in the log (the
        // F3 network graphs do the same thing on every tick, `debug-f3.md`).
        if rc_data::env_flag!("RUSTCRAFT_PINGTEST") {
            self.exact.ticks += 1;
            let link = &self.session.link;
            if self.exact.ticks.is_multiple_of(20) {
                let _ = link.send_ping_request(now_ms());
            }
            for d in link.stats.take_pongs() {
                eprintln!("ping: {d} ms");
            }
        }
    }
}

/// State of this module in `App`.
#[derive(Default)]
pub(crate) struct Exact {
    ticks: u64,
    /// Server rate and freeze (`ticking.rs`).
    pub rate: crate::ticking::TickRate,
    /// Entity followed by the camera (`set_camera`), `None`: the player.
    pub camera: Option<i32>,
    /// Block particles of explosions, for the next tick.
    pub explosions: crate::explode::Pending,
    /// Enabled features (`update_enabled_features`), empty: default (`minecraft:vanilla`).
    pub features: Vec<String>,
    /// Details for the reports (`custom_report_details`) and server links (`server_links`).
    pub report: Vec<(String, String)>,
    pub links: Vec<rc_client::extra::ServerLink>,
    /// `minecraft:dialog` registry: names and possible content.
    pub dialogs: rc_client::RegistryEntries,
    /// Tags of the `minecraft:dialog` registry (`#tag` dialog lists).
    pub dialog_tags: Vec<(String, Vec<i32>)>,
    /// Dialog shown and ticks since its opening.
    pub dialog: Option<crate::dialog::Screen>,
    pub dialog_ticks: u32,
    /// Simulation distance (`set_simulation_distance`) and difficulty (id, locked).
    pub simulation_distance: i32,
    pub difficulty: (i32, bool),
    log_since: Option<std::time::Instant>,
    log_ticks: u32,
}

/// Features known in 26.3 (§ 11).
const KNOWN_FEATURES: [&str; 4] = [
    "minecraft:vanilla",
    "minecraft:trade_rebalance",
    "minecraft:redstone_experiments",
    "minecraft:minecart_improvements",
];
