//! Sending side of the connection, shared between the network thread (service replies) and the
//! game logic (movements, inputs, end of tick). A lock suffices: each send
//! encodes a few dozen bytes.

use std::io::Write;
use std::net::TcpStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use rc_net::FrameEncoder;
use rc_proto::Encode;
use rc_proto::varint::write_varint;

use crate::ids::Ids;

/// Destination of the sends: socket, or in-memory channel to the integrated server (singleplayer) which receives
/// the packet identifier and body, without framing, compression or encryption.
pub(crate) enum Sink {
    Tcp(TcpStream),
    Memory(crossbeam_channel::Sender<Vec<u8>>),
}

struct Out {
    /// Absent outside a connection (menu screens): sends fail without effect.
    stream: Option<Sink>,
    enc: FrameEncoder,
    packet: Vec<u8>,
    wire: Vec<u8>,
}

pub struct Link {
    out: Mutex<Out>,
    pub ids: Ids,
    player_id: AtomicI32,
    /// The world is loaded on the server side (after the first accepted positioning).
    loaded: AtomicBool,
    /// Readings for the debug screen.
    pub stats: crate::netstats::NetStats,
    /// Packet log (`RUSTCRAFT_PROTOLOG`) and current phase.
    pub names: crate::protolog::Names,
    /// Cookies of the connection (`protocol-supplements.md` § 8), handed over on transfer.
    pub cookies: crate::Cookies,
    /// Secure chat: session, last seen, cache, command tree (`signing.rs`).
    pub(crate) chat: Mutex<crate::signing::ChatState>,
    /// In the play phase (chat packets only go out then).
    in_play: AtomicBool,
    /// Network thread finished.
    closed: AtomicBool,
    /// Number of the current game entry (one keys task per entry).
    generation: AtomicU64,
}

/// Content of a movement packet (the chosen packet follows the game's rule, see
/// `docs/specs/connection.md` and `movement.md`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Move {
    PosRot { pos: [f64; 3], yaw: f32, pitch: f32 },
    Pos { pos: [f64; 3] },
    Rot { yaw: f32, pitch: f32 },
    Status,
}

/// Player command actions used by the client.
pub mod action {
    /// Get out of bed.
    pub const STOP_SLEEPING: i32 = 0;
    pub const START_SPRINTING: i32 = 1;
    pub const STOP_SPRINTING: i32 = 2;
    /// Mount jump, with the strength (0..100) as data.
    pub const START_RIDING_JUMP: i32 = 3;
    /// Open the vehicle inventory.
    pub const OPEN_INVENTORY: i32 = 5;
    /// Start elytra gliding.
    pub const START_FALL_FLYING: i32 = 6;
}

impl Link {
    pub(crate) fn new(
        stream: Option<TcpStream>,
        ids: Ids,
        names: crate::protolog::Names,
        cookies: crate::Cookies,
    ) -> Self {
        Self::with_sink(stream.map(Sink::Tcp), ids, names, cookies)
    }

    pub(crate) fn with_sink(
        stream: Option<Sink>,
        ids: Ids,
        names: crate::protolog::Names,
        cookies: crate::Cookies,
    ) -> Self {
        Link {
            names,
            cookies,
            out: Mutex::new(Out { stream, enc: FrameEncoder::new(), packet: Vec::new(), wire: Vec::new() }),
            ids,
            player_id: AtomicI32::new(0),
            loaded: AtomicBool::new(false),
            stats: Default::default(),
            chat: Default::default(),
            in_play: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        }
    }

    pub(crate) fn send(&self, id: i32, body: impl FnOnce(&mut Vec<u8>)) -> std::io::Result<()> {
        self.stats.sent.fetch_add(1, Ordering::Relaxed);
        let mut o = self.out.lock().unwrap_or_else(|p| p.into_inner());
        let Out { stream, enc, packet, wire } = &mut *o;
        let Some(stream) = stream else { return Err(std::io::ErrorKind::NotConnected.into()) };
        packet.clear();
        write_varint(packet, id);
        body(packet);
        if crate::protolog::enabled() {
            self.names.sent(id, packet.len());
        }
        match stream {
            Sink::Tcp(stream) => {
                wire.clear();
                enc.encode(packet, wire).map_err(std::io::Error::other)?;
                stream.write_all(wire)
            }
            Sink::Memory(tx) => tx.send(packet.clone()).map_err(|_| std::io::ErrorKind::BrokenPipe.into()),
        }
    }

    /// Sending of a play packet whose body is written by the caller (special screens).
    pub fn send_raw(&self, id: i32, body: impl FnOnce(&mut Vec<u8>)) -> std::io::Result<()> {
        self.send(id, body)
    }

    /// Cuts the connection (unreadable packet): the network thread sees the end of the stream and stops.
    /// In memory: the channel is closed, the integrated server sees the player leave.
    pub(crate) fn shutdown(&self) {
        let mut o = self.out.lock().unwrap_or_else(|p| p.into_inner());
        match &o.stream {
            Some(Sink::Tcp(s)) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            Some(Sink::Memory(_)) => o.stream = None,
            None => {}
        }
    }

    /// Disconnection wanted by the player: closing on the client side, without a leave packet
    /// (`option-menus.md` § 2.4).
    pub fn disconnect(&self) {
        self.shutdown();
    }

    pub(crate) fn set_compression(&self, threshold: i32) {
        self.out.lock().unwrap_or_else(|p| p.into_inner()).enc.set_compression(threshold);
    }

    /// AES-128 CFB8 encryption of the sends, to enable right after the encryption response.
    pub(crate) fn enable_encryption(&self, secret: &[u8; 16]) {
        self.out.lock().unwrap_or_else(|p| p.into_inner()).enc.enable_encryption(secret);
    }

    pub(crate) fn set_player_id(&self, id: i32) {
        self.player_id.store(id, Ordering::Relaxed);
    }

    /// `resource_pack` reply (state of a server pack), with the identifier of the current phase.
    pub fn send_pack_status(&self, id: u128, status: i32) -> std::io::Result<()> {
        let packet = match self.names.phase() {
            rc_data::Phase::Configuration => self.ids.cfg_resource_pack_out,
            rc_data::Phase::Play => self.ids.resource_pack_out,
            _ => return Ok(()),
        };
        self.send(packet, |w| {
            id.encode(w);
            write_varint(w, status);
        })
    }

    /// Tells the server that the world around the player is loaded (once), like the game
    /// when the loading screen closes: to call when the player's chunk is received.
    pub fn send_player_loaded(&self) -> std::io::Result<()> {
        if self.loaded.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.send(self.ids.player_loaded, |_| {})
    }

    /// New world to load (respawn packet): `send_player_loaded` will fire again at the end.
    pub fn expect_load(&self) {
        self.loaded.store(false, Ordering::Release);
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded.load(Ordering::Acquire)
    }

    pub fn send_move(&self, m: Move, on_ground: bool, horizontal_collision: bool) -> std::io::Result<()> {
        let flags = u8::from(on_ground) | (u8::from(horizontal_collision) << 1);
        let pos = |w: &mut Vec<u8>, p: [f64; 3]| p.iter().for_each(|v| v.encode(w));
        let ids = &self.ids;
        match m {
            Move::PosRot { pos: p, yaw, pitch } => self.send(ids.move_pos_rot, |w| {
                pos(w, p);
                yaw.encode(w);
                pitch.encode(w);
                w.push(flags);
            }),
            Move::Pos { pos: p } => self.send(ids.move_pos, |w| {
                pos(w, p);
                w.push(flags);
            }),
            Move::Rot { yaw, pitch } => self.send(ids.move_rot, |w| {
                yaw.encode(w);
                pitch.encode(w);
                w.push(flags);
            }),
            Move::Status => self.send(ids.move_status, |w| w.push(flags)),
        }
    }

    /// Keys: forward 1, back 2, left 4, right 8, jump 16, sneak 32, sprint 64.
    pub fn send_input(&self, keys: u8) -> std::io::Result<()> {
        self.send(self.ids.player_input, |w| w.push(keys))
    }

    pub fn send_player_command(&self, action: i32) -> std::io::Result<()> {
        self.send_player_command_data(action, 0)
    }

    /// Player command with its data (mount jump strength).
    pub fn send_player_command_data(&self, action: i32, data: i32) -> std::io::Result<()> {
        let id = self.player_id.load(Ordering::Relaxed);
        self.send(self.ids.player_command, |w| {
            write_varint(w, id);
            write_varint(w, action);
            write_varint(w, data);
        })
    }

    /// Creative flight toggle reported to the server.
    pub fn send_abilities(&self, flying: bool) -> std::io::Result<()> {
        self.send(self.ids.abilities_out, |w| rc_proto_v777::play::write_abilities(w, flying))
    }

    /// Teleport acknowledgement with the player's final position and rotation.
    pub fn send_accept_teleport(&self, id: i32, pos: [f64; 3], yaw: f32, pitch: f32) -> std::io::Result<()> {
        self.send(self.ids.accept_teleportation, |w| {
            rc_proto_v777::play::write_accept_teleportation(w, id, pos, yaw, pitch)
        })
    }

    /// Chat message: signed if a chat session exists (online server), last seen
    /// acknowledged (`chat.md` § 2.1, § 2.4); otherwise unsigned, as before.
    pub fn send_chat(&self, message: &str) -> std::io::Result<()> {
        use crate::signing;
        let mut chat = self.chat.lock().unwrap_or_else(|p| p.into_inner());
        let (now, salt) = (signing::now_ms(), signing::random_i64());
        let u = chat.last_seen.update();
        let player = chat.player;
        let sig = chat.session.as_mut().and_then(|s| s.sign(player, salt, now, message, &u.signatures));
        self.send(self.ids.chat_out, |w| signing::write_chat(w, message, now, salt, sig.as_ref(), &u))
    }

    /// Command, without the leading `/`. With signable arguments (command tree received,
    /// `commands.md` § 2.6): `chat_command_signed`, signatures absent without a session; otherwise
    /// `chat_command`. Parsing without the game's registries: the chat screen goes through
    /// `send_command_args` with its own full parsing.
    pub fn send_command(&self, command: &str) -> std::io::Result<()> {
        let tree = self.chat.lock().unwrap_or_else(|p| p.into_inner()).commands.clone();
        let args = tree.map(|t| crate::commands_signable(&t, command)).unwrap_or_default();
        self.send_command_args(command, &args)
    }

    /// Command with its signable arguments (name, text) already found.
    pub fn send_command_args(&self, command: &str, args: &[(String, String)]) -> std::io::Result<()> {
        use crate::signing;
        let mut chat = self.chat.lock().unwrap_or_else(|p| p.into_inner());
        if args.is_empty() {
            return self.send(self.ids.chat_command_out, |w| rc_proto::write_str(w, command));
        }
        let (now, salt) = (signing::now_ms(), signing::random_i64());
        let u = chat.last_seen.update();
        let player = chat.player;
        let sigs: Vec<(String, signing::Sig)> = match chat.session.as_mut() {
            Some(s) => args
                .iter()
                .filter_map(|(name, value)| Some((name.clone(), s.sign(player, salt, now, value, &u.signatures)?)))
                .collect(),
            None => Vec::new(),
        };
        self.send(self.ids.chat_command_signed, |w| signing::write_command_signed(w, command, now, salt, &sigs, &u))
    }

    /// Entering the game (`login`): chat reset; returns the number of this entry.
    pub(crate) fn chat_reset(&self, player: u128, online: bool, enforced: bool) -> u64 {
        let mut chat = self.chat.lock().unwrap_or_else(|p| p.into_inner());
        *chat = crate::signing::ChatState { player, online, enforced, ..Default::default() };
        self.in_play.store(true, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Back to configuration: acknowledgement of the pending messages (`chat.md` § 2.4).
    pub(crate) fn leave_play(&self) -> std::io::Result<()> {
        self.in_play.store(false, Ordering::Release);
        let ack = self.chat.lock().unwrap_or_else(|p| p.into_inner()).last_seen.take_ack();
        match ack {
            Some(o) => self.send(self.ids.chat_ack, |w| write_varint(w, o)),
            None => Ok(()),
        }
    }

    /// Player message received: last seen decompacted by the cache (returned for the
    /// signature verification), then the cache updated with them and the signature.
    pub(crate) fn chat_unpack(&self, m: &rc_proto_v777::play::PlayerChat) -> Result<Vec<crate::signing::Sig>, String> {
        let mut chat = self.chat.lock().unwrap_or_else(|p| p.into_inner());
        let mut list = Vec::with_capacity(m.last_seen.len() + 1);
        for e in &m.last_seen {
            list.push(match e {
                Ok(s) => *s,
                Err(i) => {
                    *chat.cache.get(*i).ok_or_else(|| crate::reason::key("multiplayer.disconnect.invalid_packet"))?
                }
            });
        }
        let last_seen = list.clone();
        if let Some(s) = &m.signature {
            list.push(**s);
        }
        chat.cache.push(&list);
        Ok(last_seen)
    }

    /// Signed message processed (displayed or not): tracking of the last seen; sends `chat_ack`
    /// beyond 64 unacknowledged messages.
    pub(crate) fn chat_processed(&self, sig: &crate::signing::Sig, shown: bool) -> Result<(), String> {
        let ack = {
            let mut chat = self.chat.lock().unwrap_or_else(|p| p.into_inner());
            if chat.last_seen.add(sig, shown) { chat.last_seen.take_ack() } else { None }
        };
        if let Some(o) = ack {
            self.send(self.ids.chat_ack, |w| write_varint(w, o)).map_err(|e| crate::reason::from_io(&e))?;
        }
        Ok(())
    }

    /// `delete_chat`: compacted signature; a still-pending message is no longer acknowledged.
    pub(crate) fn chat_deleted(&self, packed: Result<crate::signing::Sig, u32>) -> Result<crate::signing::Sig, String> {
        let mut chat = self.chat.lock().unwrap_or_else(|p| p.into_inner());
        let sig = match packed {
            Ok(s) => s,
            Err(i) => *chat.cache.get(i).ok_or_else(|| crate::reason::key("multiplayer.disconnect.invalid_packet"))?,
        };
        chat.last_seen.ignore_pending(&sig);
        Ok(sig)
    }

    /// Command tree received.
    pub(crate) fn set_commands(&self, c: &rc_proto_v777::commands::Commands) {
        let tree = std::sync::Arc::new(rc_commands::Tree::new(c));
        self.chat.lock().unwrap_or_else(|p| p.into_inner()).commands = Some(tree);
    }

    /// New key pair: new session (random UUID, index 0) announced to the server.
    pub(crate) fn start_chat_session(&self, keys: std::sync::Arc<crate::signing::ProfileKeys>) -> std::io::Result<()> {
        let mut chat = self.chat.lock().unwrap_or_else(|p| p.into_inner());
        if !chat.online || chat.session.as_ref().is_some_and(|s| std::sync::Arc::ptr_eq(&s.keys, &keys)) {
            return Ok(());
        }
        let s = crate::signing::ChatSession::new(keys);
        let r = self.send(self.ids.chat_session_update, |w| crate::signing::write_session_update(w, &s));
        if r.is_ok() {
            crate::auth::authlog("chat session announced (chat_session_update)");
            chat.session = Some(s);
        }
        r
    }

    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Must the keys task of game entry `generation` continue?
    pub(crate) fn key_task_alive(&self, generation: u64) -> bool {
        !self.closed.load(Ordering::Acquire) && self.generation.load(Ordering::Acquire) == generation
    }

    pub fn in_play(&self) -> bool {
        self.in_play.load(Ordering::Acquire)
    }

    /// Suggestions request to the server (`commands.md` § 3.2).
    pub fn send_command_suggestion(&self, id: i32, text: &str) -> std::io::Result<()> {
        self.send(self.ids.command_suggestion_out, |w| rc_proto_v777::commands::write_command_suggestion(w, id, text))
    }

    /// Client information in game (settings changed, `option-menus.md` § 4.4).
    pub fn send_client_information(&self, info: &rc_proto_v777::play::ClientInformation) -> std::io::Result<()> {
        self.send(self.ids.client_information_out, |w| rc_proto_v777::play::write_client_information(w, info))
    }

    /// Held hotbar slot (`hud.md` § 6.2).
    pub fn send_carried_item(&self, slot: i16) -> std::io::Result<()> {
        self.send(self.ids.set_carried_item_out, |w| slot.encode(w))
    }

    /// Item picked in a bundle with the wheel (menu slot, index or −1).
    pub fn send_bundle_item_selected(&self, slot: i32, index: i32) -> std::io::Result<()> {
        self.send(self.ids.bundle_item_selected_out, |w| {
            write_varint(w, slot);
            write_varint(w, index);
        })
    }

    /// Client command: 0 respawn, 1 statistics, 2 game rules.
    pub fn send_client_command(&self, action: i32) -> std::io::Result<()> {
        self.send(self.ids.client_command_out, |w| write_varint(w, action))
    }

    /// Progress (`advancements-stats.md` § 1.4): `Some(root)` tab open, `None` screen closed.
    pub fn send_seen_advancements(&self, tab: Option<&str>) -> std::io::Result<()> {
        self.send(self.ids.seen_advancements_out, |w| rc_proto_v777::advancements::write_seen_advancements(w, tab))
    }

    /// Closing of the container screen (`inventories.md` § 1.2).
    pub fn send_container_close(&self, window: i32) -> std::io::Result<()> {
        self.send(self.ids.container_close_out, |w| rc_proto_v777::container::write_container_close(w, window))
    }

    /// Placing of a slot of menu 0 (1 to 45) or throw (−1) in creative (`creative-inventory.md` § 7.1):
    /// stack with the "untrusted" codec (components delimited by length).
    pub fn send_set_creative_slot(
        &self,
        slot: i16,
        stack: Option<&rc_proto_v777::item::ItemStack>,
    ) -> std::io::Result<()> {
        self.send(self.ids.set_creative_mode_slot, |w| {
            slot.encode(w);
            match stack.filter(|s| s.count > 0) {
                None => write_varint(w, 0),
                Some(s) => {
                    write_varint(w, s.count);
                    write_varint(w, s.item);
                    write_varint(w, s.added.len() as i32);
                    write_varint(w, s.removed.len() as i32);
                    for (ty, b) in &s.added {
                        write_varint(w, i32::from(*ty));
                        write_varint(w, b.len() as i32);
                        w.extend_from_slice(b);
                    }
                    for ty in &s.removed {
                        write_varint(w, i32::from(*ty));
                    }
                }
            }
        })
    }

    /// Book settings (`creative-inventory.md` § 9.6): 0 crafting table, 1 furnace, 2 blast
    /// furnace, 3 smoker.
    pub fn send_recipe_book_settings(&self, book: i32, open: bool, filter: bool) -> std::io::Result<()> {
        self.send(self.ids.recipe_book_change_settings, |w| {
            write_varint(w, book);
            open.encode(w);
            filter.encode(w);
        })
    }

    /// Recipe seen (leaves the "new" ones).
    pub fn send_recipe_seen(&self, id: i32) -> std::io::Result<()> {
        self.send(self.ids.recipe_book_seen_recipe, |w| write_varint(w, id))
    }

    /// Placing of a recipe in the grid (`max`: Shift held).
    pub fn send_place_recipe(&self, window: i32, id: i32, max: bool) -> std::io::Result<()> {
        self.send(self.ids.place_recipe, |w| {
            write_varint(w, window);
            write_varint(w, id);
            max.encode(w);
        })
    }

    /// Click in a container (`inventories.md` § 1.6).
    #[allow(clippy::too_many_arguments)]
    pub fn send_container_click(
        &self,
        window: i32,
        state: i32,
        slot: i16,
        button: i8,
        mode: i32,
        changed: &[(i16, Option<rc_proto_v777::container::HashedStack>)],
        carried: Option<&rc_proto_v777::container::HashedStack>,
    ) -> std::io::Result<()> {
        self.send(self.ids.container_click_out, |w| {
            rc_proto_v777::container::write_container_click(w, window, state, slot, button, mode, changed, carried)
        })
    }

    /// Use of the held item on a block (opens containers).
    pub fn send_use_item_on(
        &self,
        pos: [i32; 3],
        face: i32,
        at: [f32; 3],
        inside: bool,
        seq: i32,
    ) -> std::io::Result<()> {
        self.send_use_item_on_hand(0, pos, face, at, inside, seq)
    }

    /// `use_item_on` with the hand (0 main, 1 off), `interactions.md` § 3.2.
    pub fn send_use_item_on_hand(
        &self,
        hand: i32,
        pos: [i32; 3],
        face: i32,
        at: [f32; 3],
        inside: bool,
        seq: i32,
    ) -> std::io::Result<()> {
        self.send(self.ids.use_item_on, |w| {
            rc_proto_v777::container::write_use_item_on(w, hand, pos, face, at, inside, seq)
        })
    }

    /// `use_item` (§ 3.3): hand, sequence, current rotation.
    pub fn send_use_item(&self, hand: i32, seq: i32, yaw: f32, pitch: f32) -> std::io::Result<()> {
        self.send(self.ids.use_item, |w| rc_proto_v777::interact::write_use_item(w, hand, seq, yaw, pitch))
    }

    /// `player_action` (§ 0): action, block, face, sequence.
    pub fn send_player_action(&self, action: i32, pos: [i32; 3], face: u8, seq: i32) -> std::io::Result<()> {
        self.send(self.ids.player_action, |w| rc_proto_v777::interact::write_player_action(w, action, pos, face, seq))
    }

    /// Attack on an entity (§ 4.1).
    pub fn send_attack(&self, entity: i32) -> std::io::Result<()> {
        self.send(self.ids.attack, |w| write_varint(w, entity))
    }

    /// Interaction with an entity (§ 4.2): point relative to the feet.
    pub fn send_interact(&self, entity: i32, hand: i32, at: [f64; 3], sneaking: bool) -> std::io::Result<()> {
        self.send(self.ids.interact, |w| rc_proto_v777::interact::write_interact(w, entity, hand, at, sneaking))
    }

    /// Left click (swing seen by others, attack cooldown reset).
    pub fn send_punch(&self) -> std::io::Result<()> {
        self.send(self.ids.punch, |_| {})
    }

    /// Spectator left click (§ 1.7).
    pub fn send_spectator_action(&self, entity: Option<i32>) -> std::io::Result<()> {
        self.send(self.ids.spectator_action, |w| rc_proto_v777::interact::write_spectator_action(w, entity))
    }

    /// Middle click on a block (§ 6).
    pub fn send_pick_block(&self, pos: [i32; 3], with_data: bool) -> std::io::Result<()> {
        self.send(self.ids.pick_item_from_block, |w| rc_proto_v777::interact::write_pick_block(w, pos, with_data))
    }

    /// Middle click on an entity (§ 6).
    pub fn send_pick_entity(&self, entity: i32, with_data: bool) -> std::io::Result<()> {
        self.send(self.ids.pick_item_from_entity, |w| rc_proto_v777::interact::write_pick_entity(w, entity, with_data))
    }

    /// Position of the driven vehicle (`protocol-supplements.md` § 7).
    pub fn send_move_vehicle(&self, pos: [f64; 3], yaw: f32, pitch: f32, on_ground: bool) -> std::io::Result<()> {
        self.send(self.ids.move_vehicle_out, |w| {
            pos.iter().for_each(|v| v.encode(w));
            yaw.encode(w);
            pitch.encode(w);
            on_ground.encode(w);
        })
    }

    /// Paddles of the driven boat, every tick.
    pub fn send_paddle_boat(&self, left: bool, right: bool) -> std::io::Result<()> {
        self.send(self.ids.paddle_boat, |w| {
            left.encode(w);
            right.encode(w);
        })
    }

    /// Custom action of a dialog (§ 12): identifier and optional NBT payload.
    pub fn send_custom_click_action(
        &self,
        configuration: bool,
        id: &str,
        payload: Option<&rc_proto::nbt::Nbt>,
    ) -> std::io::Result<()> {
        let packet = if configuration { self.ids.cfg_custom_click_action } else { self.ids.custom_click_action };
        let mut body = Vec::new();
        rc_proto::nbt::write(&mut body, payload);
        self.send(packet, |w| {
            rc_proto::write_str(w, id);
            write_varint(w, body.len() as i32);
            w.extend_from_slice(&body);
        })
    }

    pub fn send_tick_end(&self) -> std::io::Result<()> {
        self.send(self.ids.client_tick_end, |_| {})
    }

    /// Ping request carrying the local time (ms); the reply sends it back (ping graph).
    pub fn send_ping_request(&self, time_ms: i64) -> std::io::Result<()> {
        self.send(self.ids.ping_request, |w| time_ms.encode(w))?;
        // Integrated server: the game replies on the I/O thread, without waiting for its tick.
        let memory = matches!(self.out.lock().unwrap_or_else(|p| p.into_inner()).stream, Some(Sink::Memory(_)));
        if memory {
            self.stats.push_pong(time_ms);
        }
        Ok(())
    }

    /// Debug subscriptions (identifiers of the `debug_subscription` registry); empty: none.
    pub fn send_debug_subscriptions(&self, ids: &[i32]) -> std::io::Result<()> {
        self.send(self.ids.debug_subscription_request, |w| {
            write_varint(w, ids.len() as i32);
            for &i in ids {
                write_varint(w, i);
            }
        })
    }

    /// Game mode change requested (0 survival, 1 creative, 2 adventure, 3 spectator).
    pub fn send_change_game_mode(&self, mode: i32) -> std::io::Result<()> {
        self.send(self.ids.change_game_mode, |w| write_varint(w, mode))
    }

    /// Spectator: teleport to the entity (player) with this UUID.
    pub fn send_teleport_to_entity(&self, uuid: u128) -> std::io::Result<()> {
        self.send(self.ids.teleport_to_entity, |w| w.extend_from_slice(&uuid.to_be_bytes()))
    }

    /// Difficulty requested (0 peaceful … 3 hard), world options.
    pub fn send_change_difficulty(&self, difficulty: i32) -> std::io::Result<()> {
        self.send(self.ids.change_difficulty_out, |w| write_varint(w, difficulty))
    }

    /// Difficulty lock (always true on the game side).
    pub fn send_lock_difficulty(&self, locked: bool) -> std::io::Result<()> {
        self.send(self.ids.lock_difficulty_out, |w| locked.encode(w))
    }

    /// Game rules modified: (identifier `minecraft:…`, value as text).
    pub fn send_set_game_rules(&self, rules: &[(String, String)]) -> std::io::Result<()> {
        self.send(self.ids.set_game_rule_out, |w| {
            write_varint(w, rules.len() as i32);
            for (k, v) in rules {
                rc_proto::write_str(w, k);
                rc_proto::write_str(w, v);
            }
        })
    }

    /// Code of conduct accepted (configuration).
    pub fn send_accept_code_of_conduct(&self) -> std::io::Result<()> {
        self.send(self.ids.cfg_accept_code_of_conduct, |_| {})
    }

    /// Block entity data request (F3+I): transaction, position.
    pub fn send_block_entity_tag_query(&self, transaction: i32, pos: [i32; 3]) -> std::io::Result<()> {
        self.send(self.ids.block_entity_tag_query, |w| {
            write_varint(w, transaction);
            rc_proto::BlockPos::new(pos[0], pos[1], pos[2]).encode(w);
        })
    }

    /// Entity data request (F3+I): transaction, entity.
    pub fn send_entity_tag_query(&self, transaction: i32, entity: i32) -> std::io::Result<()> {
        self.send(self.ids.entity_tag_query, |w| {
            write_varint(w, transaction);
            write_varint(w, entity);
        })
    }
}
