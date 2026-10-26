//! Opening the world to the local network (mission S9, `docs/specs/integrated-server.md` § 10):
//! TCP listening (real protocol, compression, offline), multicast announcement, guests handled by
//! the same game code as the host (player state swapped, `ctx.rs`), events routed to
//! the host (memory link) and the guests (encoded packets, `encode.rs`).

pub mod announce;
pub mod auth;
pub mod conn;
pub mod ctx;
pub mod encode;
pub mod recipes;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_g3f05;
#[cfg(test)]
mod tests_g3f07;

use hashbrown::{HashMap, HashSet};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crossbeam_channel::{Receiver, Sender};
use rc_client::{ChatTag, EntityEvent, Event};
use rc_proto::varint::write_varint;
use rc_proto_v777::players::{InfoEntry, InfoUpdate, action};

use crate::player::Stack;
use crate::server::Server;

/// Opening request coming from the client (world options).
#[derive(Debug, Clone)]
pub struct Publish {
    /// Open (`false`: close to the local network).
    pub open: bool,
    /// Wanted port (0: free port chosen by the system).
    pub port: u16,
    /// Commands allowed to the guests.
    pub guest_commands: bool,
    /// Mode imposed on the guests (`None`: the one from their file or the world).
    pub force_mode: Option<i32>,
}

/// Connected guest.
pub struct Guest {
    pub name: String,
    pub uuid: u128,
    pub out: Sender<Vec<u8>>,
    pub inp: Receiver<Vec<u8>>,
    /// Player state; during a swap, the host's (readable by the guest's code).
    pub ctx: Option<Box<ctx::PlayerCtx>>,
    pub enc: RefCell<encode::EncState>,
    pub gone: bool,
    pub joined: bool,
    /// Verified profile and connection keep-alive.
    pub net: GuestNet,
    /// Requested view distance (`client_information`), before the host's cap.
    pub req_view: i32,
    /// Chunk throughput (client acknowledgements).
    pub chunks: ChunkFlow,
    /// Speed check: position at the start of the tick, movement packets received in total and
    /// at the start of the tick.
    pub moves: ([f64; 3], i32, i32),
}

/// Chunk throughput of a network connection, like the game: batches capped by the client's
/// wanted rate (acknowledgement of each batch), a single batch without acknowledgement at the start, ten after.
#[derive(Debug, Clone, Copy)]
pub struct ChunkFlow {
    pub desired: f32,
    pub quota: f32,
    pub unacked: i32,
    pub max_unacked: i32,
}

impl Default for ChunkFlow {
    fn default() -> Self {
        ChunkFlow { desired: 9.0, quota: 0.0, unacked: 0, max_unacked: 1 }
    }
}

impl ChunkFlow {
    /// Size of this tick's batch (0: nothing to send).
    pub fn batch(&mut self) -> usize {
        if self.unacked >= self.max_unacked {
            return 0;
        }
        self.quota = (self.quota + self.desired).min(self.desired.max(1.0));
        if self.quota < 1.0 { 0 } else { self.quota.floor() as usize }
    }

    /// Batch of `n` chunks sent.
    pub fn sent(&mut self, n: usize) {
        self.unacked += 1;
        self.quota -= n as f32;
    }

    /// Client acknowledgement with its wanted rate (chunks per tick).
    pub fn acked(&mut self, desired: f32) {
        self.unacked = (self.unacked - 1).max(0);
        self.desired = if desired.is_nan() { 0.01 } else { desired.clamp(0.01, 64.0) };
        if self.unacked == 0 {
            self.quota = 1.0;
        }
        self.max_unacked = 10;
    }
}

/// Profile and connection keep-alive of a guest.
#[derive(Default)]
pub struct GuestNet {
    /// Profile properties (signed skin); empty offline.
    pub properties: Vec<rc_proto_v777::players::Property>,
    /// Smoothed latency (ms), like the game: (old × 3 + measurement) / 4.
    pub latency: i32,
    /// Last keep-alive send (`None`: not yet counted).
    pub keep_time: Option<std::time::Instant>,
    /// Keep-alive challenge awaiting a reply.
    pub pending: Option<i64>,
    /// The player's choice to appear in server lists.
    pub allow_listing: bool,
}

/// Player-list actions on a player's arrival (like those of the host, `join`).
const INFO_ADD: u8 =
    action::ADD_PLAYER | action::UPDATE_GAME_MODE | action::UPDATE_LISTED | action::UPDATE_LATENCY | action::UPDATE_HAT;

/// Interval of the connection keep-alive (and reply timeout) of the game.
const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(15);
/// Ticks between two sends of the latencies to everyone.
const LATENCY_INTERVAL: u64 = 600;

/// Permission level of a guest of the integrated server: game master (2) when the world allows
/// commands and the opening grants them to the guests, otherwise none (a world without commands
/// only reads the operator list, empty for a singleplayer world).
pub(crate) fn guest_level(allow_commands: bool, guest_commands: bool) -> u8 {
    if allow_commands && guest_commands { 2 } else { 0 }
}

/// Other player seen from the current player: their seat (`None`: the host), position, sneaking, dead,
/// game mode.
pub(crate) type OtherPlayer = (Option<usize>, [f64; 3], bool, bool, i32);

/// Entity event that announces a permission level (24 + level).
pub(crate) fn op_event(op: u8) -> i8 {
    24 + op.min(4) as i8
}

/// Open world.
pub struct Lan {
    pub port: u16,
    pub shared: Arc<conn::Shared>,
    joined: Receiver<conn::Joined>,
    pub guests: Vec<Guest>,
    /// Guest whose state is installed (`None`: the host).
    pub active: Option<usize>,
    pub guest_op: u8,
    pub force_mode: Option<i32>,
    /// Last broadcast position of each player (entity id).
    seen: HashMap<i32, ([f64; 3], f32, f32, bool, bool)>,
    /// Last broadcast equipment of each player.
    equip: HashMap<i32, Vec<Option<Stack>>>,
    /// Paired players: (observer, player shown), entity ids.
    pairs: HashSet<(i32, i32)>,
    /// Command trees per level (packet body).
    commands: Vec<Vec<u8>>,
    /// Stop of the multicast announcement.
    announce_stop: Arc<std::sync::atomic::AtomicBool>,
    /// Origin of the keep-alive challenges (milliseconds since opening).
    epoch: std::time::Instant,
    /// Ticks since the last send of the latencies.
    latency_ticks: u64,
}

/// Event to broadcast to all players (otherwise: to the current player only).
fn is_world(sv: &Server, e: &Event) -> bool {
    match e {
        Event::BlockUpdate { .. }
        | Event::SectionUpdate { .. }
        | Event::BlockDestruction { .. }
        | Event::BlockEvent { .. }
        | Event::BlockEntityData { .. }
        | Event::LightUpdate { .. }
        | Event::Sound(_)
        | Event::Particles(_)
        | Event::LevelEvent { .. }
        | Event::Explode(_)
        | Event::Score(_)
        | Event::Boss(_)
        | Event::Border(_)
        | Event::PlayerInfo(_)
        | Event::PlayersRemoved(_)
        | Event::TickingState { .. }
        | Event::TickingStep(_)
        | Event::Time { .. }
        | Event::Difficulty { .. }
        | Event::SpawnPosition { .. }
        | Event::ChunkBiomes(_)
        | Event::TabList { .. }
        | Event::Misc(_) => true,
        Event::Entity(ee) => match ee {
            EntityEvent::Event { id, event } => !(*id == sv.player_id && (22..=28).contains(event)),
            EntityEvent::Effect { id, .. } | EntityEvent::RemoveEffect { id, .. } => *id != sv.player_id,
            _ => true,
        },
        // Rain, thunder; immediate respawn and limited crafting (world rules, to everyone).
        Event::GameEvent { kind, .. } => matches!(kind, 1 | 2 | 7 | 8 | 11 | 12),
        Event::Attributes { entity, .. } => *entity != sv.player_id,
        Event::Chat { component, tag } => *tag == ChatTag::NotSecure || world_key(component),
        _ => false,
    }
}

/// System messages broadcast by the game to all players (connections, advancements, deaths).
fn world_key(c: &rc_text::Component) -> bool {
    match &c.content {
        rc_text::Content::Translate { key, .. } => {
            key.starts_with("multiplayer.player.")
                || key.starts_with("chat.type.advancement.")
                || key.starts_with("death.")
                || key == "chat.type.announcement"
                || key == "chat.type.emote"
        }
        _ => false,
    }
}

/// Body of the `commands` packet of a level (extracted tree, `commands.bin`).
pub(crate) fn commands_body(sv: &Server, op: u8) -> Option<Vec<u8>> {
    if let Some(l) = &sv.lan {
        return l.commands.get(usize::from(op.min(4))).cloned();
    }
    let bytes = std::fs::read(rc_data::commands_path(&sv.data.version)).ok()?;
    split_levels(&bytes).get(usize::from(op.min(4))).cloned()
}

fn split_levels(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        let n = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        i += 4;
        let Some(body) = bytes.get(i..i + n) else { break };
        out.push(body.to_vec());
        i += n;
    }
    out
}

/// Advancement and statistics packets.
pub(crate) fn progress_packets(
    sv: &Server,
    ids: &rc_client::ids::Ids,
    p: &rc_client::progress::Progress,
    out: &mut Vec<Vec<u8>>,
) {
    use rc_client::progress::Progress;
    use rc_proto::Encode;
    let _ = sv;
    let mut w = Vec::new();
    match p {
        Progress::Advancements(u) => {
            write_varint(&mut w, ids.update_advancements);
            u.reset.encode(&mut w);
            write_varint(&mut w, u.added.len() as i32);
            for a in &u.added {
                rc_proto::write_str(&mut w, &a.id);
                a.parent.is_some().encode(&mut w);
                if let Some(p) = &a.parent {
                    rc_proto::write_str(&mut w, p);
                }
                a.display.is_some().encode(&mut w);
                if let Some(d) = &a.display {
                    rc_proto::nbt::write(&mut w, d.title.as_ref());
                    rc_proto::nbt::write(&mut w, d.description.as_ref());
                    encode::write_template(&mut w, &d.icon);
                    write_varint(&mut w, i32::from(d.frame));
                    let flags = i32::from(d.background.is_some()) | i32::from(d.toast) << 1 | i32::from(d.hidden) << 2;
                    flags.encode(&mut w);
                    if let Some(b) = &d.background {
                        rc_proto::write_str(&mut w, b);
                    }
                }
                write_varint(&mut w, a.requirements.len() as i32);
                for g in &a.requirements {
                    write_varint(&mut w, g.len() as i32);
                    for n in g {
                        rc_proto::write_str(&mut w, n);
                    }
                }
                false.encode(&mut w); // telemetry
                // 26.3: position out of the display, after the telemetry.
                a.x.encode(&mut w);
                a.y.encode(&mut w);
            }
            write_varint(&mut w, u.removed.len() as i32);
            for r in &u.removed {
                rc_proto::write_str(&mut w, r);
            }
            write_varint(&mut w, u.progress.len() as i32);
            for (id, crits) in &u.progress {
                rc_proto::write_str(&mut w, id);
                write_varint(&mut w, crits.len() as i32);
                for (n, at) in crits {
                    rc_proto::write_str(&mut w, n);
                    at.is_some().encode(&mut w);
                    if let Some(t) = at {
                        t.encode(&mut w);
                    }
                }
            }
            u.show.encode(&mut w);
        }
        Progress::SelectTab(t) => {
            write_varint(&mut w, ids.select_advancements_tab);
            t.is_some().encode(&mut w);
            if let Some(t) = t {
                rc_proto::write_str(&mut w, t);
            }
        }
        Progress::Stats(list) => {
            write_varint(&mut w, ids.award_stats);
            write_varint(&mut w, list.len() as i32);
            for (t, v, n) in list {
                write_varint(&mut w, *t);
                write_varint(&mut w, *v);
                write_varint(&mut w, *n);
            }
        }
    }
    out.push(w);
}

impl Server {
    /// Routing of an event when the world is open: host (memory link) and guests.
    pub(crate) fn lan_route(&self, e: Event) -> bool {
        let Some(l) = &self.lan else { return self.peer.send(e) };
        if !is_world(self, &e) {
            return self.lan_send_current(l, e);
        }
        let mut packets = Vec::new();
        self.encode_event(&mut encode::EncState::default(), &e, &mut packets);
        for (i, g) in l.guests.iter().enumerate() {
            if !g.joined && Some(i) != l.active {
                continue;
            }
            for p in &packets {
                let _ = g.out.send(p.clone());
            }
        }
        self.peer.send(e)
    }

    /// Event to the current player only (host or installed guest).
    fn lan_send_current(&self, l: &Lan, e: Event) -> bool {
        let Some(i) = l.active else { return self.peer.send(e) };
        let mut packets = Vec::new();
        self.encode_event(&mut l.guests[i].enc.borrow_mut(), &e, &mut packets);
        let g = &l.guests[i];
        for p in packets {
            let _ = g.out.send(p);
        }
        true
    }

    /// Event to the current player only, even of a kind usually broadcast to everyone (team
    /// message). Outside the current world (`muted`), kept like the other messages.
    pub(crate) fn send_to_current(&self, e: Event) -> bool {
        if self.muted {
            self.cmd.deferred.borrow_mut().push(e);
            return false;
        }
        match &self.lan {
            Some(l) => self.lan_send_current(l, e),
            None => self.peer.send(e),
        }
    }

    /// Players in game other than the current player, in player-list order (host,
    /// then guests by arrival): seat for `as_player` (`None`: the host) and name.
    pub(crate) fn lan_other_players(&self) -> Vec<(Option<usize>, String)> {
        let Some(l) = &self.lan else { return Vec::new() };
        let mut v = Vec::new();
        for (k, g) in l.guests.iter().enumerate() {
            if Some(k) == l.active {
                // Its seat holds the host during the swap.
                if let Some(c) = &g.ctx {
                    v.insert(0, (None, c.name.clone()));
                }
            } else if g.joined {
                v.push((Some(k), g.name.clone()));
            }
        }
        v
    }

    /// Opening to the local network (`publish`): listening, announcement; returns the port.
    pub(crate) fn lan_publish(&mut self, req: &Publish) -> Result<u16, String> {
        if self.lan.is_some() {
            return Err("already open".into());
        }
        let level_name = self.level.name.clone();
        let motd = format!("{} - {}", self.name, level_name);
        let tags = self.regs.tags.clone();
        let shared = Arc::new(conn::Shared {
            data: self.data.clone(),
            ids: *self.peer.ids(),
            motd: motd.clone(),
            max_players: 8,
            online: std::sync::atomic::AtomicUsize::new(1),
            names: std::sync::Mutex::new(vec![(self.name.clone(), self.uuid, true)]),
            synced: self.regs.synced.clone(),
            tags,
            stop: std::sync::atomic::AtomicBool::new(false),
            icon: conn::world_icon(&self.world),
            keys: std::sync::OnceLock::new(),
            host: (self.name.clone(), self.uuid),
            status: std::sync::Mutex::new(None),
            features: self.features.clone(),
        });
        if rc_data::env_str!("RUSTCRAFT_LAN_AUTH") == Some("0") {
            auth::VERIFY.store(false, Ordering::Relaxed);
        }
        let (tx, rx) = crossbeam_channel::unbounded();
        let port = conn::listen(req.port, shared.clone(), tx).map_err(|e| e.to_string())?;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        announce::start(motd, port, stop.clone());
        let commands =
            std::fs::read(rc_data::commands_path(&self.data.version)).map(|b| split_levels(&b)).unwrap_or_default();
        self.lan = Some(Box::new(Lan {
            port,
            shared,
            joined: rx,
            guests: Vec::new(),
            active: None,
            guest_op: guest_level(self.level.allow_commands, req.guest_commands),
            force_mode: req.force_mode,
            seen: HashMap::new(),
            equip: HashMap::new(),
            pairs: HashSet::new(),
            commands,
            announce_stop: stop,
            epoch: std::time::Instant::now(),
            latency_ticks: 0,
        }));
        eprintln!("singleplayer: world opened to the local network on port {port}");
        Ok(port)
    }

    /// Runs `f` with the state of guest `i` installed.
    pub(crate) fn as_guest<T>(&mut self, i: usize, f: impl FnOnce(&mut Server) -> T) -> T {
        if !self.install_guest(i) {
            return f(self);
        }
        let r = f(self);
        self.uninstall_guest(i);
        r
    }

    /// Runs `f` with the state of player `who` installed (`None`: the host), whatever the
    /// current player, who is restored afterwards (a hit from one player on another).
    pub(crate) fn as_player<T>(&mut self, who: Option<usize>, f: impl FnOnce(&mut Server) -> T) -> T {
        let cur = self.lan.as_ref().and_then(|l| l.active);
        if cur == who {
            return f(self);
        }
        if let Some(i) = cur {
            self.uninstall_guest(i);
        }
        let r = match who {
            None => f(self),
            Some(k) => self.as_guest(k, f),
        };
        if let Some(i) = cur {
            self.install_guest(i);
        }
        r
    }

    /// Installs the state of guest `i` (the current host); false if it has no state.
    fn install_guest(&mut self, i: usize) -> bool {
        let Some(mut c) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)).and_then(|g| g.ctx.take()) else {
            return false;
        };
        self.swap_player(&mut c);
        // During the swap, the guest's seat keeps the host's state.
        if let Some(l) = &mut self.lan {
            l.guests[i].ctx = Some(c);
            l.active = Some(i);
        }
        true
    }

    /// Restores the host after `install_guest(i)`.
    fn uninstall_guest(&mut self, i: usize) {
        let Some(mut c) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)).and_then(|g| g.ctx.take()) else {
            return;
        };
        if let Some(l) = &mut self.lan {
            l.active = None;
        }
        self.swap_player(&mut c);
        if let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) {
            g.ctx = Some(c);
        }
    }

    /// Player of entity `id` other than the current player: their seat (`None`: the host), their
    /// position, sneaking, dead, game mode.
    /// Player seated in minecart `cart`, in the form expected by `as_player` (`None`:
    /// the host); `None` if no player is seated in it.
    pub(crate) fn cart_rider(&self, cart: i32) -> Option<Option<usize>> {
        let active = self.lan.as_ref().and_then(|l| l.active);
        if self.pcart.vehicle == Some(cart) {
            return Some(active);
        }
        let l = self.lan.as_ref()?;
        let k = l.guests.iter().position(|g| g.ctx.as_ref().is_some_and(|c| c.pcart.vehicle == Some(cart)))?;
        Some(if Some(k) == active { None } else { Some(k) })
    }

    pub(crate) fn lan_player(&self, id: i32) -> Option<OtherPlayer> {
        let l = self.lan.as_ref()?;
        l.guests.iter().enumerate().find_map(|(k, g)| {
            let c = g.ctx.as_ref().filter(|c| c.player_id == id && (g.joined || Some(k) == l.active))?;
            // The active guest's seat holds the host.
            let who = if Some(k) == l.active { None } else { Some(k) };
            Some((who, c.pos, c.sneaking, c.vit.dead, c.game_mode))
        })
    }

    /// Network loop: new guests, guest packets, departures.
    pub(crate) fn lan_poll(&mut self) {
        let Some(l) = &self.lan else { return };
        let mut new = Vec::new();
        while let Ok(j) = l.joined.try_recv() {
            new.push(j);
        }
        for j in new {
            self.lan_join(j);
        }
        let n = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..n {
            let mut packets = Vec::new();
            let mut gone = false;
            if let Some(g) = self.lan.as_ref().and_then(|l| l.guests.get(i)) {
                loop {
                    match g.inp.try_recv() {
                        Ok(p) => packets.push(p),
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            gone = true;
                            break;
                        }
                    }
                }
            }
            // Keep-alive replies, chunk acknowledgements, echo: handled by the connection.
            packets.retain(|p| !self.lan_keep_alive_answer(i, p) && !self.lan_connection_packet(i, p));
            if !packets.is_empty() {
                self.as_guest(i, |sv| {
                    for p in packets {
                        sv.dispatch_packet(&p);
                    }
                });
            }
            if gone && let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) {
                g.gone = true;
            }
        }
        // Departures.
        let gone: Vec<usize> = self
            .lan
            .as_ref()
            .map(|l| l.guests.iter().enumerate().filter(|(_, g)| g.gone).map(|(i, _)| i).collect())
            .unwrap_or_default();
        for i in gone.into_iter().rev() {
            self.lan_leave(i);
        }
    }

    /// Packet from a client (host or current guest).
    pub(crate) fn dispatch_packet(&mut self, p: &[u8]) {
        if self.progress_packet(p) || self.life_packet(p) || self.mobs_packet(p) || self.commands_packet(p) {
            return;
        }
        match crate::input::decode(self.peer.ids(), p) {
            Ok(i) => self.handle(i),
            Err(e) => eprintln!("singleplayer: unreadable packet: {e}"),
        }
    }

    fn lan_join(&mut self, j: conn::Joined) {
        // Same profile already in game: the game kicks the old connection, then lets the
        // new one in.
        if let Some(old) = self.lan.as_ref().and_then(|l| l.guests.iter().position(|g| g.uuid == j.uuid)) {
            self.lan_kick(old, "multiplayer.disconnect.duplicate_login");
            self.lan_leave(old);
        }
        let (op, force) = self.lan.as_ref().map_or((0, None), |l| (l.guest_op, l.force_mode));
        // Requested view capped by the server's (the host's).
        let req_view = i32::from(j.info.view_distance);
        let view = req_view.clamp(2, self.lan_server_view());
        let mut ctx = self.new_player_ctx(&j.name, j.uuid, view, op, force);
        ctx.model = (j.info.skin_parts, j.info.main_hand);
        let id = ctx.player_id;
        let ctx_pos = ctx.pos;
        let Some(l) = &mut self.lan else { return };
        l.guests.push(Guest {
            name: j.name.clone(),
            uuid: j.uuid,
            out: j.out,
            inp: j.inp,
            ctx: Some(Box::new(ctx)),
            enc: RefCell::new(encode::EncState::default()),
            gone: false,
            joined: false,
            net: GuestNet {
                properties: j.properties,
                allow_listing: j.info.allow_listing,
                keep_time: Some(std::time::Instant::now()),
                ..Default::default()
            },
            req_view,
            chunks: ChunkFlow::default(),
            moves: (ctx_pos, 0, 0),
        });
        l.shared.online.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut n) = l.shared.names.lock() {
            n.push((j.name.clone(), j.uuid, j.info.allow_listing));
        }
        let i = l.guests.len() - 1;
        eprintln!("lan: {} joined the game (entity {id})", j.name);
        // Guest enters the game: like the host, then the other players and entities.
        self.as_guest(i, |sv| {
            sv.join_player();
            sv.lan_show_others();
        });
        if let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) {
            g.joined = true;
        }
        // The others see them: player list, entity.
        self.lan_announce_player(i);
        // One more player in the world: sleepers list refreshed.
        self.update_sleeping_list();
    }

    pub(crate) fn lan_leave(&mut self, i: usize) {
        let Some(name) = self.lan.as_ref().and_then(|l| l.guests.get(i)).map(|g| g.name.clone()) else { return };
        // Forced wake-up of the sleeper (bed freed, list refreshed after the removal), save of the
        // player, then removal for the others.
        let (uuid, id, shown) = self.as_guest(i, |sv| {
            sv.stop_sleep_in_bed(true, false);
            sv.award_custom("leave_game", 1);
            let mut jobs = Vec::new();
            jobs.push(crate::save::Job::Dat {
                path: sv.pdata.path.clone(),
                nbt: crate::nbtio::encode_root(&sv.player_nbt()),
                backup: true,
            });
            sv.progress_save_jobs(&mut jobs);
            let w = sv.writer.get_or_insert_with(crate::save::Writer::new);
            w.send(jobs);
            sv.close_menu(true);
            // Written to their file, their vehicle and their pearls leave with them.
            sv.unload_with_leaving_player();
            (sv.uuid, sv.player_id, sv.player_display_name())
        });
        let Some(l) = &mut self.lan else { return };
        l.guests.remove(i);
        l.shared.online.fetch_sub(1, Ordering::AcqRel);
        if let Ok(mut n) = l.shared.names.lock() {
            n.retain(|x| x.1 != uuid);
        }
        l.seen.remove(&id);
        l.equip.remove(&id);
        l.pairs.retain(|&(a, b)| a != id && b != id);
        let _ = self.send(Event::Entity(EntityEvent::Remove(vec![id])));
        let _ = self.send(Event::PlayersRemoved(vec![uuid]));
        let mut left = rc_text::Component::translate("multiplayer.player.left", vec![rc_text::Arg::Component(shown)]);
        left.style.color = Some(0xFFFF55);
        let _ = self.send(Event::Chat { component: left, tag: ChatTag::System });
        self.update_sleeping_list();
        eprintln!("lan: {name} left the game");
    }

    /// Entry of a player (current context): singleplayer's `join`, player listed on their side.
    fn join_player(&mut self) {
        self.join();
    }

    /// On the current player's side (arriving guest): the other players (list and entities) and the
    /// mobs, items and orbs already there.
    #[allow(clippy::type_complexity)]
    fn lan_show_others(&mut self) {
        let Some(l) = &self.lan else { return };
        let me = l.active;
        let mut players: Vec<(u128, String, i32, u8)> = Vec::new();
        // The host: their state is in the guest's seat during the swap.
        for (k, g) in l.guests.iter().enumerate() {
            if Some(k) != me && !g.joined {
                continue;
            }
            let Some(c) = &g.ctx else { continue };
            players.push((c.uuid, c.name.clone(), c.game_mode, c.model.0));
        }
        let entries = players
            .iter()
            .map(|(u, n, m, skin)| InfoEntry {
                uuid: *u,
                profile: Some((n.clone(), self.profile_properties(*u))),
                game_mode: Some(*m),
                listed: Some(true),
                latency: Some(self.lan_latency(*u)),
                show_hat: Some(skin & crate::play::HAT != 0),
                ..Default::default()
            })
            .collect();
        // Player entities: added by their tracking (`lan_broadcast_players`).
        self.send_personal(Event::PlayerInfo(InfoUpdate { actions: INFO_ADD, entries }));
        // Mobs and items tracked by the host (broadcast afterwards to everyone).
        let mobs: Vec<(
            i32,
            u128,
            i32,
            [f64; 3],
            f32,
            f32,
            f32,
            Vec<(u8, rc_proto_v777::entity::Meta)>,
            Vec<Option<Stack>>,
        )> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|m| m.track.seen && m.alive())
            .map(|m| {
                (
                    m.id,
                    m.uuid,
                    self.entity_kind_id(m.kind.info().name),
                    m.pos,
                    m.yaw,
                    m.pitch,
                    m.head_yaw,
                    m.non_default(),
                    m.equipment.to_vec(),
                )
            })
            .collect();
        for (id, uuid, kind, pos, yaw, pitch, head, data, eq) in mobs {
            let add = rc_proto_v777::entity::AddEntity {
                id,
                uuid,
                kind,
                pos,
                velocity: [0.0; 3],
                pitch,
                yaw,
                head_yaw: head,
                data: 0,
            };
            self.send_personal(Event::Entity(EntityEvent::Add(Box::new(add))));
            if !data.is_empty() {
                self.send_personal(Event::Entity(EntityEvent::Data { id, values: data }));
            }
            let slots: rc_proto_v777::entity::Equipment =
                eq.iter().enumerate().filter_map(|(i, s)| Some((i as u8, self.net(Some(s.as_ref()?))))).collect();
            if !slots.is_empty() {
                self.send_personal(Event::Entity(EntityEvent::Equipment { id, slots }));
            }
        }
        let drops: Vec<(i32, [f64; 3], Stack)> =
            self.life.drops.iter().map(|d| (d.id, d.pos, d.stack.clone())).collect();
        let item_kind = self.entity_kind_id("minecraft:item");
        for (id, pos, st) in drops {
            let add = rc_proto_v777::entity::AddEntity {
                id,
                uuid: 0x5332_0000_0000_0000_0000_0000_0000_0000 | id as u32 as u128,
                kind: item_kind,
                pos,
                velocity: [0.0; 3],
                pitch: 0.0,
                yaw: 0.0,
                head_yaw: 0.0,
                data: 0,
            };
            self.send_personal(Event::Entity(EntityEvent::Add(Box::new(add))));
            let net = self.net(Some(&st));
            self.send_personal(Event::Entity(EntityEvent::Data {
                id,
                values: vec![(8, rc_proto_v777::entity::Meta::Item(net))],
            }));
        }
    }

    /// On the others' side (host included): player `i` listed and visible.
    fn lan_announce_player(&mut self, i: usize) {
        let Some(c) = self
            .lan
            .as_ref()
            .and_then(|l| l.guests.get(i))
            .and_then(|g| g.ctx.as_ref())
            .map(|c| (c.uuid, c.name.clone(), c.game_mode, c.model.0))
        else {
            return;
        };
        let (uuid, name, mode, skin) = c;
        let info = Event::PlayerInfo(InfoUpdate {
            actions: INFO_ADD,
            entries: vec![InfoEntry {
                uuid,
                profile: Some((name, self.profile_properties(uuid))),
                game_mode: Some(mode),
                listed: Some(true),
                latency: Some(self.lan_latency(uuid)),
                show_hat: Some(skin & crate::play::HAT != 0),
                ..Default::default()
            }],
        });
        // The entity follows at the first tracking that finds it in range (`lan_broadcast_players`).
        self.send_others(i, info);
    }

    /// Event to the current player only.
    pub(crate) fn send_personal(&self, e: Event) {
        let Some(l) = &self.lan else {
            let _ = self.peer.send(e);
            return;
        };
        match l.active {
            None => {
                let _ = self.peer.send(e);
            }
            Some(i) => {
                let mut out = Vec::new();
                self.encode_event(&mut l.guests[i].enc.borrow_mut(), &e, &mut out);
                for p in out {
                    let _ = l.guests[i].out.send(p);
                }
            }
        }
    }

    /// Event to the players other than the current player whose feet are within
    /// `radius` of `at` (closed world: nobody else).
    pub(crate) fn send_others_near(&self, at: [f64; 3], radius: f64, e: Event) {
        let Some(l) = &self.lan else { return };
        let near = |p: [f64; 3]| {
            let d = [at[0] - p[0], at[1] - p[1], at[2] - p[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < radius * radius
        };
        let mut out = Vec::new();
        for (k, g) in l.guests.iter().enumerate() {
            // During a swap, the active guest's context holds the host's state.
            if Some(k) == l.active || !g.joined || !g.ctx.as_ref().is_some_and(|c| near(c.pos)) {
                continue;
            }
            if out.is_empty() {
                self.encode_event(&mut encode::EncState::default(), &e, &mut out);
            }
            for p in &out {
                let _ = g.out.send(p.clone());
            }
        }
        if let Some(i) = l.active
            && l.guests[i].ctx.as_ref().is_some_and(|c| near(c.pos))
        {
            let _ = self.peer.send(e);
        }
    }

    /// The other players in game than the current player (host included during a swap):
    /// their state. Closed world: none.
    pub(crate) fn lan_others(&self) -> impl Iterator<Item = &ctx::PlayerCtx> {
        self.lan.iter().flat_map(|l| {
            l.guests
                .iter()
                .enumerate()
                .filter(move |(k, g)| g.joined || Some(*k) == l.active)
                .filter_map(|(_, g)| g.ctx.as_deref())
        })
    }

    /// Event specific to each player (built with its entity id), sent to
    /// each: host and guests in game (closed world: the current player only).
    pub(crate) fn send_each_player(&self, f: impl Fn(i32) -> Event) {
        if self.muted {
            return;
        }
        let Some(l) = &self.lan else {
            let _ = self.peer.send(f(self.player_id));
            return;
        };
        // During a swap, the active guest's seat holds the host's state.
        let id_of = |k: usize| if Some(k) == l.active { Some(self.player_id) } else { l.guests[k].ctx.as_ref().map(|c| c.player_id) };
        let host = match l.active {
            None => Some(self.player_id),
            Some(i) => l.guests[i].ctx.as_ref().map(|c| c.player_id),
        };
        if let Some(id) = host {
            let _ = self.peer.send(f(id));
        }
        for (k, g) in l.guests.iter().enumerate() {
            if !g.joined && Some(k) != l.active {
                continue;
            }
            let Some(id) = id_of(k) else { continue };
            let mut out = Vec::new();
            self.encode_event(&mut g.enc.borrow_mut(), &f(id), &mut out);
            for p in out {
                let _ = g.out.send(p);
            }
        }
    }

    /// Event to everyone except guest `skip` (host included).
    fn send_others(&self, skip: usize, e: Event) {
        let Some(l) = &self.lan else { return };
        let mut out = Vec::new();
        self.encode_event(&mut encode::EncState::default(), &e, &mut out);
        for (k, g) in l.guests.iter().enumerate() {
            if k == skip || !g.joined {
                continue;
            }
            for p in &out {
                let _ = g.out.send(p.clone());
            }
        }
        let _ = self.peer.send(e);
    }

    /// Players of the world in order of entry: (game mode, position) of the current player
    /// (the host outside a swap), then of the guests who entered.
    pub(crate) fn world_players(&self) -> Vec<(i32, [f64; 3])> {
        let mut out = vec![(self.game_mode, self.pos)];
        if let Some(l) = &self.lan {
            for (k, g) in l.guests.iter().enumerate() {
                if Some(k) != l.active && (!g.joined || g.gone) {
                    continue;
                }
                if let Some(c) = &g.ctx {
                    out.push((c.game_mode, c.pos));
                }
            }
        }
        out
    }

    /// Event built for each player from their position (feet), sent to them alone:
    /// host and guests who entered.
    pub(crate) fn send_each_player_at(&self, f: impl Fn([f64; 3]) -> Event) {
        let Some(l) = &self.lan else {
            let _ = self.peer.send(f(self.pos));
            return;
        };
        // During a swap, the current guest's seat keeps the host's state.
        let host_pos = l.active.and_then(|i| l.guests[i].ctx.as_ref()).map_or(self.pos, |c| c.pos);
        for (k, g) in l.guests.iter().enumerate() {
            if Some(k) != l.active && (!g.joined || g.gone) {
                continue;
            }
            let pos = if Some(k) == l.active { self.pos } else { g.ctx.as_ref().map_or(self.pos, |c| c.pos) };
            let mut out = Vec::new();
            self.encode_event(&mut g.enc.borrow_mut(), &f(pos), &mut out);
            for p in out {
                let _ = g.out.send(p);
            }
        }
        let _ = self.peer.send(f(host_pos));
    }

    /// Event specific to each player (action bar...) sent to everyone: host and guests who entered
    /// (the current player included during a swap).
    pub(crate) fn send_all_players(&self, e: Event) {
        let Some(l) = &self.lan else {
            let _ = self.peer.send(e);
            return;
        };
        for (k, g) in l.guests.iter().enumerate() {
            if Some(k) != l.active && (!g.joined || g.gone) {
                continue;
            }
            let mut out = Vec::new();
            self.encode_event(&mut g.enc.borrow_mut(), &e, &mut out);
            for p in out {
                let _ = g.out.send(p);
            }
        }
        let _ = self.peer.send(e);
    }

    /// Guest tick (player-specific parts) then broadcast of the positions and
    /// equipment of each player to the others.
    pub(crate) fn lan_tick(&mut self) {
        self.lan_keep_alive();
        let n = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..n {
            self.as_guest(i, |sv| sv.guest_tick());
        }
        self.lan_broadcast_players();
    }

    fn guest_tick(&mut self) {
        if let Some((_, _, at)) = self.teleport
            && self.tick > at + 20
        {
            self.teleport = self.teleport.map(|(n, p, _)| (n, p, self.tick));
            self.send_teleport();
        }
        self.tick_mining();
        self.demo_tick();
        self.vitals_tick();
        self.mobs_player_tick(); // S4: hand, memory, parrots, swimming, darts
        self.proj_player_tick(); // S8: crossbow, reloads, stuck arrows, gliding
        self.mobs_player_touch(); // S4: mount, push, slimes, pufferfish
        self.proj_player_touch(); // S8: arrows picked up
        self.spin_attack_tick(); // S8: spin attack
        self.maps_tick(); // S8: held maps
        if self.life.menu.is_some() && !self.menu_valid() {
            self.close_menu(true);
        }
        self.broadcast_menus();
        if let Some((tx, text)) = self.cmd.pending_suggest.take() {
            self.answer_suggestions(tx, &text);
        }
        self.progress_tick();
        if let Some(a) = self.ack.take() {
            let _ = self.send(Event::BlockChangedAck(a));
        }
        self.update_tracking();
        self.send_chunks();
        // Speed check: new starting point and packet count of the next tick.
        let pos = self.pos;
        if let Some(g) = self.lan.as_mut().and_then(|l| l.active.and_then(|i| l.guests.get_mut(i))) {
            g.moves = (pos, g.moves.1, g.moves.1);
        }
    }

    /// The game's speed check for a guest (skipped for the world owner): the
    /// movement packet is counted; returns true if the wanted position (`None`: rotation
    /// only) strays too far from the tick's starting point (squared, minus the server's speed
    /// squared: more than 100 per tick packet, 300 when gliding), world rules permitting.
    pub(crate) fn lan_move_too_fast(&mut self, target: Option<[f64; 3]>) -> bool {
        // Passenger (mob, minecart, cushion): handled separately by the game, neither counted nor checked.
        let passenger =
            self.mobs.mounts.riding.vehicle.is_some() || self.pcart.vehicle.is_some() || self.deco.seat.is_some();
        if !self.cmd.ticking.runs() || passenger {
            return false;
        }
        let Some(g) = self.lan.as_mut().and_then(|l| l.active.and_then(|i| l.guests.get_mut(i))) else {
            return false;
        };
        g.moves.1 += 1;
        let mut packets = g.moves.1 - g.moves.2;
        if packets > 5 {
            packets = 1;
        }
        let start = g.moves.0;
        let gliding = self.proj_player.fall_flying;
        if !self.rule("minecraft:player_movement_check", true)
            || (gliding && !self.rule("minecraft:elytra_movement_check", true))
        {
            return false;
        }
        let t = target.unwrap_or(self.pos);
        let d = [t[0] - start[0], t[1] - start[1], t[2] - start[2]];
        let moved = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let v = self.mobs.player_vel;
        let expected = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        let per_packet = if gliding { 300.0 } else { 100.0 };
        moved - expected > per_packet * f64::from(packets)
    }

    /// Tracking of players among themselves, like the game's entity tracking: a player is shown to
    /// another if they are in range (32 chunks capped by the observer's view, horizontal
    /// distance), in a chunk the observer has received, and visible to them (a spectator
    /// is only seen by spectators; a spectator sees every player who is not looking through
    /// another entity). Pairing: add, skin parts, main arm, pose, equipment;
    /// then every tick positions, pose and equipment to the paired observers only.
    fn lan_broadcast_players(&mut self) {
        let Some(l) = &self.lan else { return };
        let eq = |slots: &[Option<Stack>], held: i32| -> Vec<Option<Stack>> {
            vec![
                slots.get(36 + held as usize).cloned().flatten(),
                slots.get(45).cloned().flatten(),
                slots.get(8).cloned().flatten(),
                slots.get(7).cloned().flatten(),
                slots.get(6).cloned().flatten(),
                slots.get(5).cloned().flatten(),
            ]
        };
        let own_camera = |c: Option<crate::commands::ents::E>| c.is_none_or(|e| e == crate::commands::ents::E::Player);
        let mut list = vec![Shown {
            who: None,
            id: self.player_id,
            uuid: self.uuid,
            pos: self.pos,
            yaw: self.yaw,
            pitch: self.pitch,
            ground: self.on_ground,
            sneak: self.sneaking,
            spectator: self.game_mode == 3,
            own_camera: own_camera(self.prog.cmds.camera),
            view: self.view,
            model: self.model,
            equip: eq(&self.slots, self.held),
            sent: &self.sent,
        }];
        for (k, g) in l.guests.iter().enumerate() {
            if !g.joined {
                continue;
            }
            let Some(c) = &g.ctx else { continue };
            list.push(Shown {
                who: Some(k),
                id: c.player_id,
                uuid: c.uuid,
                pos: c.pos,
                yaw: c.yaw,
                pitch: c.pitch,
                ground: c.on_ground,
                sneak: c.sneaking,
                spectator: c.game_mode == 3,
                own_camera: own_camera(c.prog.cmds.camera),
                view: c.view,
                model: c.model,
                equip: eq(&c.slots, c.held),
                sent: &c.sent,
            });
        }
        let mut out: Vec<(Option<usize>, EntityEvent)> = Vec::new();
        let mut pairs = HashSet::new();
        for v in &list {
            for t in list.iter().filter(|t| t.id != v.id) {
                let was = l.pairs.contains(&(v.id, t.id));
                if v.sees(t) {
                    pairs.insert((v.id, t.id));
                    if !was {
                        out.extend(self.pair_player(t).into_iter().map(|e| (v.who, e)));
                    }
                } else if was {
                    out.push((v.who, EntityEvent::Remove(vec![t.id])));
                }
            }
        }
        // Changes since the last tick, to the already paired observers.
        let mut seen = Vec::with_capacity(list.len());
        for t in &list {
            let (id, pos, yaw, pitch, ground, sneak) = (t.id, t.pos, t.yaw, t.pitch, t.ground, t.sneak);
            let prev = l.seen.get(&id).copied();
            let mut evs = Vec::new();
            if prev.is_none_or(|p| p.0 != pos || p.1 != yaw || p.2 != pitch || p.3 != ground) {
                evs.push(EntityEvent::Teleport(rc_proto_v777::entity::TeleportEntity {
                    id,
                    pos,
                    velocity: [0.0; 3],
                    yaw,
                    pitch,
                    relative: 0,
                    on_ground: ground,
                }));
                evs.push(EntityEvent::Head { id, yaw });
            }
            if prev.is_none_or(|p| p.4 != sneak) {
                let pose = if sneak { 5 } else { 0 };
                evs.push(EntityEvent::Data { id, values: vec![(6, rc_proto_v777::entity::Meta::Pose(pose))] });
            }
            if l.equip.get(&id) != Some(&t.equip) {
                let slots: rc_proto_v777::entity::Equipment =
                    t.equip.iter().enumerate().map(|(i, s)| (i as u8, self.net(s.as_ref()))).collect();
                evs.push(EntityEvent::Equipment { id, slots });
            }
            for v in &list {
                if pairs.contains(&(v.id, id)) && l.pairs.contains(&(v.id, id)) {
                    out.extend(evs.iter().map(|e| (v.who, e.clone())));
                }
            }
            seen.push((id, (pos, yaw, pitch, ground, sneak), t.equip.clone()));
        }
        drop(list);
        if let Some(l) = &mut self.lan {
            l.pairs = pairs;
            for (id, s, e) in seen {
                l.seen.insert(id, s);
                l.equip.insert(id, e);
            }
        }
        for (who, e) in out {
            self.send_to(who, Event::Entity(e));
        }
    }

    /// Pairing of a player at an observer: add, skin parts and main arm
    /// other than default values (none, right), sneaking pose, worn equipment.
    fn pair_player(&self, t: &Shown<'_>) -> Vec<EntityEvent> {
        use rc_proto_v777::entity::Meta;
        let Event::Entity(add) = add_player(self, t.id, t.uuid, t.pos, t.yaw, t.pitch) else { return Vec::new() };
        let mut evs = vec![add];
        let mut values = Vec::new();
        if t.sneak {
            values.push((6, Meta::Pose(5)));
        }
        if t.model.1 != 1 {
            values.push((15, Meta::Variant(t.model.1, 42)));
        }
        if t.model.0 != 0 {
            values.push((16, Meta::Byte(t.model.0 as i8)));
        }
        if !values.is_empty() {
            evs.push(EntityEvent::Data { id: t.id, values });
        }
        let slots: rc_proto_v777::entity::Equipment =
            t.equip.iter().enumerate().filter_map(|(i, s)| Some((i as u8, self.net(Some(s.as_ref()?))))).collect();
        if !slots.is_empty() {
            evs.push(EntityEvent::Equipment { id: t.id, slots });
        }
        evs
    }

    /// Event to a single player: the host (`None`) or guest `k`.
    fn send_to(&self, who: Option<usize>, e: Event) {
        match who {
            None => {
                let _ = self.peer.send(e);
            }
            Some(k) => {
                let Some(g) = self.lan.as_ref().and_then(|l| l.guests.get(k)) else { return };
                let mut out = Vec::new();
                self.encode_event(&mut encode::EncState::default(), &e, &mut out);
                for p in out {
                    let _ = g.out.send(p);
                }
            }
        }
    }

    /// Guest files written with the save.
    pub(crate) fn lan_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        let n = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..n {
            let mut j = self.as_guest(i, |sv| {
                let mut v = vec![crate::save::Job::Dat {
                    path: sv.pdata.path.clone(),
                    nbt: crate::nbtio::encode_root(&sv.player_nbt()),
                    backup: true,
                }];
                sv.progress_save_jobs(&mut v);
                v
            });
            jobs.append(&mut j);
        }
    }

    /// World options request: close, (re)open, guest settings. The message
    /// to the player is that of the game's screen (`menu.multiplayerOptions.publish.*`).
    pub(crate) fn lan_requests(&mut self) {
        let req = self.publish_req.lock().ok().and_then(|mut g| g.take());
        if let Some(req) = req {
            let port_changed = self.lan.as_ref().is_some_and(|l| req.port != 0 && req.port != l.port);
            if self.lan.is_some() && (!req.open || port_changed) {
                self.lan_unpublish();
                self.publish_message(rc_text::Component::translate(
                    "menu.multiplayerOptions.publish.stopped",
                    Vec::new(),
                ));
            }
            let op = guest_level(self.level.allow_commands, req.guest_commands);
            if let Some(l) = &mut self.lan {
                let changed = l.guest_op != op;
                l.guest_op = op;
                l.force_mode = req.force_mode;
                if changed {
                    self.lan_send_permissions();
                }
            } else if req.open {
                let msg = match self.lan_publish(&req) {
                    Ok(port) => rc_text::Component::translate(
                        "menu.multiplayerOptions.publish.started.lan",
                        vec![rc_text::Arg::Component(crate::commands::text::copy_text(&port.to_string()))],
                    ),
                    Err(e) => {
                        eprintln!("lan : ouverture impossible : {e}");
                        rc_text::Component::translate("commands.publish.failed", Vec::new())
                    }
                };
                self.publish_message(msg);
            }
        }
        if self.lan.is_some() {
            self.lan_poll();
        }
    }

    /// Permissions returned to each player after a change of command access for the guests:
    /// level event (24 + level) and command tree, host included.
    fn lan_send_permissions(&mut self) {
        let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event: crate::lan::op_event(self.cmd.op) }));
        self.send_commands();
        let (n, op) = self.lan.as_ref().map_or((0, 0), |l| (l.guests.len(), l.guest_op));
        for i in 0..n {
            self.as_guest(i, |sv| {
                sv.cmd.op = op;
                let _ = sv.send(Event::Entity(EntityEvent::Event { id: sv.player_id, event: crate::lan::op_event(op) }));
                sv.send_commands();
            });
        }
    }

    fn publish_message(&self, c: rc_text::Component) {
        let _ = self.send(Event::Chat { component: c, tag: ChatTag::System });
    }

    /// Close requested in game: guests kicked and saved, world back to singleplayer.
    pub(crate) fn lan_unpublish(&mut self) {
        self.lan_close();
        // Departures in order of arrival, for the players who entered the game only; a
        // connection that is not yet in it is closed without a "left" message.
        while let Some(i) = self.lan.as_ref().and_then(|l| l.guests.iter().position(|g| g.joined)) {
            self.lan_leave(i);
        }
        self.lan = None;
    }

    /// `/publish [allowCommands] [port]`: reserved to the host; free port by default.
    #[allow(clippy::result_large_err)]
    pub(crate) fn cmd_publish(
        &mut self,
        s: &crate::commands::src::Src,
        p: &crate::commands::Parsed,
        seg: &crate::commands::Seg,
    ) -> crate::commands::R {
        use crate::commands::src::Fail;
        if let Some(l) = &self.lan {
            let port = rc_text::Arg::Text(l.port.to_string());
            return Err(Fail(rc_text::Component::translate("commands.publish.alreadyPublished.lan", vec![port])));
        }
        let guest_commands = p.arg(seg, "allowCommands").is_some_and(|b| b == "true");
        let port = p.arg(seg, "port").and_then(|x| x.parse().ok()).unwrap_or(0);
        let req = Publish { open: true, port, guest_commands, force_mode: None };
        match self.lan_publish(&req) {
            Ok(port) => {
                let c = rc_text::Component::translate(
                    "commands.publish.started.lan",
                    vec![rc_text::Arg::Component(crate::commands::text::copy_text(&port.to_string()))],
                );
                self.ok(s, c);
                Ok(i32::from(port))
            }
            Err(_) => Err(Fail(rc_text::Component::translate("commands.publish.failed", Vec::new()))),
        }
    }

    /// View distance of the server: the host's (closed world: the player's), capped
    /// to 2..32 for an open world.
    pub(crate) fn lan_server_view(&self) -> i32 {
        let Some(l) = &self.lan else { return self.view };
        let host = l.active.and_then(|i| l.guests[i].ctx.as_ref()).map_or(self.view, |c| c.view);
        host.clamp(2, 32)
    }

    /// View distance requested by the current player. Guest: capped by the server's,
    /// without a packet (the server announces its own). Host: it is the server's, announced to
    /// everyone; each guest's view is recomputed.
    pub(crate) fn on_view_request(&mut self, view: i32) {
        if let Some(i) = self.lan.as_ref().and_then(|l| l.active) {
            if let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) {
                g.req_view = view;
            }
            let v = view.clamp(2, self.lan_server_view());
            if v != self.view {
                self.view = v;
                self.center = None;
                self.update_tracking();
            }
            return;
        }
        let v = view.max(2);
        if v == self.view {
            return;
        }
        self.view = v;
        let _ = self.send(Event::ViewDistance(v));
        self.center = None;
        self.update_tracking();
        let server = self.lan_server_view();
        let reqs: Vec<i32> = self.lan.as_ref().map(|l| l.guests.iter().map(|g| g.req_view).collect()).unwrap_or_default();
        for (i, req) in reqs.into_iter().enumerate() {
            self.as_guest(i, |sv| {
                let _ = sv.send(Event::ViewDistance(server));
                let nv = req.clamp(2, server);
                if nv != sv.view {
                    sv.view = nv;
                    sv.center = None;
                    sv.update_tracking();
                }
            });
        }
    }

    /// Sending of the pending chunks of the current guest `i`, like a network connection of the
    /// game: a batch (start, nearest chunks, end with their count) capped by the rate
    /// the client acknowledges; a chunk being generated stays pending.
    pub(crate) fn lan_send_chunks(&mut self, i: usize) {
        let Some(n) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)).map(|g| g.chunks.batch()) else { return };
        if n == 0 || self.queue.is_empty() {
            return;
        }
        let (cx, cz) = self.chunk_pos();
        let mut q = std::mem::take(&mut self.queue);
        q.sort_by_key(|&(x, z)| (x - cx) * (x - cx) + (z - cz) * (z - cz));
        let rest = q.split_off(n.min(q.len()));
        self.ensure_loaded(&q);
        let mut chunks = Vec::new();
        let mut shown = Vec::new();
        for p in q {
            if self.generator.as_ref().is_some_and(|g| g.pending.contains(&p)) {
                self.queue.push(p);
                continue;
            }
            self.sent.insert(p);
            let Some(Some(c)) = self.chunks.get(&p) else { continue };
            chunks.push(self.chunk_packet(c));
            shown.push(p);
        }
        self.queue.extend(rest);
        if chunks.is_empty() {
            return;
        }
        let packets = self.chunk_batch(&chunks);
        if let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) {
            g.chunks.sent(chunks.len());
            for p in packets {
                let _ = g.out.send(p);
            }
        }
        for p in shown {
            self.show_entities(p);
        }
    }

    /// Connection packets handled outside the game for guest `i`: chunk batch acknowledgement
    /// (wanted rate), echo request (immediate reply). Returns true if the packet was one.
    fn lan_connection_packet(&mut self, i: usize, p: &[u8]) -> bool {
        let ids = *self.peer.ids();
        let mut r = rc_proto::Reader::new(p);
        let Ok(id) = rc_proto::varint::read_varint(&mut r) else { return false };
        let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) else { return false };
        if id == ids.chunk_batch_received {
            if let Ok(d) = <f32 as rc_proto::Decode>::decode(&mut r) {
                g.chunks.acked(d);
            }
            true
        } else if id == ids.ping_request {
            if let Ok(t) = <i64 as rc_proto::Decode>::decode(&mut r) {
                let mut w = Vec::new();
                write_varint(&mut w, ids.pong_response);
                w.extend_from_slice(&t.to_be_bytes());
                let _ = g.out.send(w);
            }
            true
        } else {
            false
        }
    }

    /// Latency of a listed player (0 for the host, like the game's memory link).
    pub(crate) fn lan_latency(&self, uuid: u128) -> i32 {
        self.lan.as_ref().and_then(|l| l.guests.iter().find(|g| g.uuid == uuid)).map_or(0, |g| g.net.latency)
    }

    /// Properties of a guest's verified profile (empty offline or unknown).
    pub(crate) fn lan_properties(&self, uuid: u128) -> Vec<rc_proto_v777::players::Property> {
        self.lan
            .as_ref()
            .and_then(|l| l.guests.iter().find(|g| g.uuid == uuid))
            .map(|g| g.net.properties.clone())
            .unwrap_or_default()
    }

    /// Kicks guest `i` with the message `key`; their departure is handled the next turn.
    pub(crate) fn lan_kick(&mut self, i: usize, key: &str) {
        self.lan_kick_with(i, rc_text::Component::translate(key, Vec::new()));
    }

    /// Kick of guest `i` with any reason: disconnect packet, departure at the
    /// next guest pickup.
    pub(crate) fn lan_kick_with(&mut self, i: usize, msg: rc_text::Component) {
        let mut out = Vec::new();
        self.encode_event(&mut encode::EncState::default(), &Event::Kicked(msg), &mut out);
        if let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) {
            for p in out {
                let _ = g.out.send(p);
            }
            g.gone = true;
        }
    }

    /// Connection keep-alive of the guests, like the game: a challenge every 15 s; no
    /// reply to the previous challenge → `disconnect.timeout`. Latencies sent to everyone every
    /// 600 ticks.
    fn lan_keep_alive(&mut self) {
        let ids = *self.peer.ids();
        let now = std::time::Instant::now();
        let mut timeout = Vec::new();
        let Some(l) = &mut self.lan else { return };
        let epoch = l.epoch;
        for (i, g) in l.guests.iter_mut().enumerate() {
            let t = *g.net.keep_time.get_or_insert(now);
            if g.gone || now.duration_since(t) < KEEP_ALIVE {
                continue;
            }
            if g.net.pending.is_some() {
                timeout.push(i);
            } else {
                let challenge = now.duration_since(epoch).as_millis() as i64;
                g.net.pending = Some(challenge);
                g.net.keep_time = Some(now);
                let mut w = Vec::new();
                write_varint(&mut w, ids.keep_alive);
                w.extend_from_slice(&challenge.to_be_bytes());
                let _ = g.out.send(w);
            }
        }
        l.latency_ticks += 1;
        let send_latency = l.latency_ticks > LATENCY_INTERVAL;
        if send_latency {
            l.latency_ticks = 0;
        }
        for i in timeout {
            self.lan_kick(i, "disconnect.timeout");
        }
        if send_latency && let Some(l) = &self.lan {
            let mut entries = vec![InfoEntry { uuid: self.uuid, latency: Some(0), ..Default::default() }];
            entries.extend(l.guests.iter().filter(|g| g.joined && !g.gone).map(|g| InfoEntry {
                uuid: g.uuid,
                latency: Some(g.net.latency),
                ..Default::default()
            }));
            let e = Event::PlayerInfo(InfoUpdate { actions: action::UPDATE_LATENCY, entries });
            self.send_others(usize::MAX, e);
        }
    }

    /// Keep-alive reply of guest `i`: smoothed latency; unexpected challenge → kicked.
    /// Returns true if the packet was one.
    fn lan_keep_alive_answer(&mut self, i: usize, p: &[u8]) -> bool {
        let ids = *self.peer.ids();
        let mut r = rc_proto::Reader::new(p);
        if rc_proto::varint::read_varint(&mut r).ok() != Some(ids.keep_alive_out) {
            return false;
        }
        let Ok(id) = <i64 as rc_proto::Decode>::decode(&mut r) else { return true };
        let mut bad = false;
        if let Some(g) = self.lan.as_mut().and_then(|l| l.guests.get_mut(i)) {
            match (g.net.pending, g.net.keep_time) {
                (Some(c), Some(t)) if c == id => {
                    let ms = t.elapsed().as_millis() as i32;
                    g.net.latency = (g.net.latency * 3 + ms) / 4;
                    g.net.pending = None;
                }
                _ => bad = true,
            }
        }
        if bad {
            self.lan_kick(i, "disconnect.timeout");
        }
        true
    }

    /// Close: guests disconnected, listening and announcement stopped.
    pub(crate) fn lan_close(&mut self) {
        if let Some(l) = &self.lan {
            l.announce_stop.store(true, Ordering::Release);
            l.shared.stop.store(true, Ordering::Release);
            let msg = rc_text::Component::translate("multiplayer.disconnect.server_shutdown", Vec::new());
            let mut out = Vec::new();
            self.encode_event(&mut encode::EncState::default(), &Event::Kicked(msg), &mut out);
            for g in &l.guests {
                for p in &out {
                    let _ = g.out.send(p.clone());
                }
            }
        }
    }
}

/// Tracking range of a player (chunks), that of the entity type.
const PLAYER_TRACKING: i32 = 32;

/// Player as the tracking of the others sees them (state of this tick).
struct Shown<'a> {
    /// `None`: the host; otherwise the guest.
    who: Option<usize>,
    id: i32,
    uuid: u128,
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    ground: bool,
    sneak: bool,
    spectator: bool,
    /// Looks through their own eyes (no spectator camera on another entity).
    own_camera: bool,
    view: i32,
    model: (u8, i32),
    equip: Vec<Option<Stack>>,
    /// Chunks received by this player.
    sent: &'a HashSet<(i32, i32)>,
}

impl Shown<'_> {
    /// Does this player (observer) see `t`?
    fn sees(&self, t: &Shown<'_>) -> bool {
        let range = f64::from(PLAYER_TRACKING * 16).min(f64::from(self.view) * 16.0);
        let (dx, dz) = (t.pos[0] - self.pos[0], t.pos[2] - self.pos[2]);
        let chunk = ((t.pos[0].floor() as i32) >> 4, (t.pos[2].floor() as i32) >> 4);
        let shown = if self.spectator { t.own_camera } else { !t.spectator };
        dx * dx + dz * dz <= range * range && shown && self.sent.contains(&chunk)
    }
}

/// "Player" entity seen by the others.
fn add_player(sv: &Server, id: i32, uuid: u128, pos: [f64; 3], yaw: f32, pitch: f32) -> Event {
    let kind = sv.entity_kind_id("minecraft:player");
    Event::Entity(EntityEvent::Add(Box::new(rc_proto_v777::entity::AddEntity {
        id,
        uuid,
        kind,
        pos,
        velocity: [0.0; 3],
        pitch,
        yaw,
        head_yaw: yaw,
        data: 0,
    })))
}

/// Raw reply (id + body) for the tests.
#[allow(dead_code)]
fn raw(id: i32, body: &[u8]) -> Vec<u8> {
    let mut w = Vec::new();
    write_varint(&mut w, id);
    w.extend_from_slice(body);
    w
}
