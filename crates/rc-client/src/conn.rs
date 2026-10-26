//! Network thread loop. Reads the socket, splits the frames, answers service packets
//! and publishes the events in order; heavy play-phase packets go
//! compressed to the pool (decompression + decoding), put back in order by the sequencer.

use std::io::Read;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Instant;

use rc_data::VersionData;
use rc_net::{FrameDecoder, Next};
use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{Decode, Encode, Reader};
use rc_proto_v777::chunk::{ChunkContext, read_chunk};
use rc_proto_v777::config::{DimensionHeight, read_login, read_registry_data, read_spawn_info};
use rc_proto_v777::play::{self, ChunkBatchRate};

use crate::ids::Ids;
use crate::link::Link;
use crate::seq::Sequencer;
use crate::{Config, Error, Event, Session};

/// Decompressed size from which a play packet goes to the pool.
const DEFER_FROM: usize = 16 * 1024;

/// Connection and read timeouts (30 s, like the game).
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Login,
    Configuration,
    Play,
}

/// Opens the connection and starts the network thread.
/// `cfg.host`/`cfg.port`: typed address. Resolution (SRV if port 25565), then the handshake
/// announces the **resolved** host and port (`docs/specs/title-screen-servers.md` § 4.1).
pub fn connect(mut cfg: Config, data: Arc<VersionData>) -> Result<Session, Error> {
    let ids = Ids::resolve(&data)?;
    // Services keys (player chat sessions), ready before entering the game.
    crate::chat_verify::prefetch_services_keys();
    let typed = crate::status::ServerAddress::new(cfg.host.clone(), cfg.port);
    let resolved =
        crate::status::resolve(&typed).ok_or_else(|| Error::Invalid(format!("unknown host: {}", cfg.host)))?;
    (cfg.host, cfg.port) = (resolved.host, resolved.port);
    // Connection timeout of 30 s per address (Netty default in the game).
    let mut last = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable);
    let mut opened = None;
    for a in &resolved.addrs {
        match TcpStream::connect_timeout(a, READ_TIMEOUT) {
            Ok(s) => {
                opened = Some(s);
                break;
            }
            Err(e) => last = e,
        }
    }
    let stream = opened.ok_or_else(|| Error::Invalid(crate::reason::from_connect(&last)))?;
    stream.set_nodelay(true)?;
    // Read timeout: the game cuts after 30 s without any data ("disconnect.timeout").
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let (tx, rx) = crossbeam_channel::unbounded();
    let seq = Arc::new(Sequencer::new(tx, |e| matches!(e, Event::Disconnected(_))));
    let d = data.clone();
    let input = stream.try_clone()?;
    let names = crate::protolog::Names::new(&data);
    let link = Arc::new(Link::new(Some(stream), ids, names, cfg.cookies.clone()));
    // Online (like the game at connection): account block list re-read in a background task.
    crate::session::start_online_mode();
    let l = link.clone();
    std::thread::Builder::new().name("network".into()).spawn(move || {
        let mut next = Emit { n: 0, bundle: None, bundled: 0 };
        let mut decoding = None;
        let r = run(input, &cfg, &l, &d, &seq, &mut next, &mut decoding);
        l.mark_closed();
        crate::session::stop_online_mode(); // block list consulted online only
        let reason = match r {
            Ok(reason) => reason,
            Err(e) => {
                // Error while decoding a packet: named as in the game.
                let name = decoding.and_then(|(phase, id)| {
                    d.packets.iter().find(|p| p.phase == phase && p.bound == rc_data::Bound::ToClient && p.id == id)
                });
                crate::reason::from_error(&e, name.map(|p| p.name.as_str()))
            }
        };
        // Bundle not closed at disconnection: never applied.
        seq.complete(next.n, Event::Disconnected(reason));
    })?;
    Ok(Session { events: rx, data, link })
}

/// Numbering of the published events, and open bundle (`bundle_delimiter`,
/// `protocol-supplements.md` § 1): its events are held then published as one block.
struct Emit {
    n: u64,
    bundle: Option<Vec<Event>>,
    /// Packets received in the open bundle.
    bundled: usize,
}

/// Maximum packets in a bundle.
const BUNDLE_MAX: usize = 4096;

impl Emit {
    fn push(&mut self, seq: &Sequencer<Event>, e: Event) {
        match &mut self.bundle {
            Some(b) => b.push(e),
            None => {
                seq.complete(self.n, e);
                self.n += 1;
            }
        }
    }
}

fn invalid(e: impl std::fmt::Display) -> Error {
    Error::Invalid(e.to_string())
}

/// Returns the reason for the end of the session (disconnection by the server or closing).
fn run(
    mut input: TcpStream,
    cfg: &Config,
    link: &Arc<Link>,
    data: &VersionData,
    seq: &Arc<Sequencer<Event>>,
    next: &mut Emit,
    decoding: &mut Option<(rc_data::Phase, i32)>,
) -> Result<String, Error> {
    use crate::extra::Handled;
    let ids = link.ids;
    let mut dec = FrameDecoder::new();
    let emit = |next: &mut Emit, e: Event| next.push(seq, e);
    let x = extra_ids(&ids);
    let dv = crate::misc::ids(&ids);
    let pg = crate::progress::ids(&ids);

    // Intent 3 for a connection opened by a transfer (§ 10).
    let intent = if cfg.transfer { 3 } else { 2 };
    link.send(ids.intention, |w| play::write_handshake(w, rc_proto_v777::PROTOCOL, &cfg.host, cfg.port, intent))?;
    link.names.set_phase(rc_data::Phase::Login);
    let uuid = cfg.uuid.unwrap_or_else(|| crate::offline_uuid(&cfg.name));
    link.send(ids.hello, |w| play::write_hello(w, &cfg.name, uuid))?;
    crate::chat::set_local(uuid);

    let mut phase = Phase::Login;
    let mut dims: Vec<Option<DimensionHeight>> = Vec::new();
    let mut biome_names: Vec<String> = Vec::new();
    let mut dim_names: Vec<String> = Vec::new();
    let mut clock_names: Vec<String> = Vec::new();
    let mut chat_types: Vec<rc_proto_v777::play::Decoration> = Vec::new();
    let mut chat_index = 0i32;
    // Known players and validation of their signed messages (`chat_verify.rs`).
    let mut remote = crate::chat_verify::Remote::new(false, Arc::new(Vec::new()));
    let names = |r: &str| data.registry(r).map(|r| r.entries.clone()).unwrap_or_default();
    let items = rc_proto_v777::item::ItemCodec::new(
        &names("minecraft:data_component_type"),
        &names("minecraft:consume_effect_type"),
    );
    let meta = rc_proto_v777::entity::MetaCodec::new(items.clone(), &names("minecraft:particle_type"));
    let heavy_items = std::sync::Arc::new(items.clone());
    let mut ctx: Option<ChunkContext> = None;
    let mut batch_start = Instant::now();
    let mut rate = ChunkBatchRate::default();
    // Large blocks: online, an encrypted buffer ≥ 128 KiB is decrypted on the pool.
    let mut buf = vec![0u8; 1024 * 1024];
    let mut packet = Vec::new();
    let states = data.states.len() as u32;

    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            return Ok(crate::reason::end_of_stream());
        }
        link.stats.count_in(n);
        dec.push(&buf[..n]);
        loop {
            *decoding = None;
            // In a bundle, everything is decoded inline: the bundle is published as one block.
            let defer =
                if phase == Phase::Play && ctx.is_some() && next.bundle.is_none() { DEFER_FROM } else { usize::MAX };
            let size = match dec.next_or_deferred(&mut packet, defer)? {
                Next::Pending => break,
                Next::Ready => None,
                Next::Deferred { size } => Some(size),
            };
            link.stats.received.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(size) = size {
                // Heavy packet: decompression and decoding on the pool, delivered at its rank.
                let (s, c, compressed, heavy) = (
                    next.n,
                    ctx.expect("context"),
                    std::mem::take(&mut packet),
                    (ids.chunk, ids.light_update, ids.commands, ids.map_item_data, ids.recipe_book_add),
                );
                let heavy_items = heavy_items.clone();
                next.n += 1;
                let (seq, link) = (seq.clone(), link.clone());
                rayon::spawn(move || {
                    let mut raw = Vec::new();
                    let ev = match rc_net::inflate(&compressed, size, &mut raw) {
                        Err(e) => Event::Disconnected(crate::reason::from_net(&e)),
                        Ok(()) => decode_heavy(&raw, heavy, &c, &heavy_items),
                    };
                    if let Event::Commands(t) = &ev {
                        link.set_commands(t);
                    }
                    // Unreadable packet: end of session, like the game (decoding error).
                    if matches!(ev, Event::Disconnected(_)) {
                        link.shutdown();
                    }
                    seq.complete(s, ev);
                });
                continue;
            }
            let mut r = Reader::new(&packet);
            let id = read_varint(&mut r)?;
            *decoding = Some((link.names.phase(), id));
            if crate::protolog::enabled() {
                link.names.received(id, packet.len());
            }
            // Packets common to the phases: cookies, resource packs (immediate replies).
            let common = match phase {
                Phase::Login => &x.login,
                Phase::Configuration => &x.cfg_common,
                Phase::Play => &x.play_common,
            };
            match crate::extra::common(link, common, id, &mut r)? {
                Handled::No => {}
                Handled::Event(e) => {
                    emit(next, e);
                    continue;
                }
                _ => continue,
            }
            // Packets specific to the phase (configuration or play) of this module.
            let own = match phase {
                Phase::Login => Handled::No,
                Phase::Configuration => crate::extra::config(&x.cfg, id, &mut r)?,
                Phase::Play => {
                    if id == ids.bundle_delimiter {
                        // Opening or closing of a bundle (§ 1).
                        match next.bundle.take() {
                            Some(b) => {
                                link.names.detail(
                                    rc_data::Bound::ToClient,
                                    id,
                                    &format!("bundle of {} packets", next.bundled),
                                );
                                emit(next, Event::Bundle(b));
                            }
                            None => (next.bundle, next.bundled) = (Some(Vec::new()), 0),
                        }
                        continue;
                    }
                    if next.bundle.is_some() {
                        next.bundled += 1;
                        if next.bundled > BUNDLE_MAX {
                            return Err(invalid(format!("bundle of more than {BUNDLE_MAX} packets")));
                        }
                        if id == ids.start_configuration {
                            return Err(invalid("phase change in a bundle"));
                        }
                    }
                    crate::extra::play(link, &x.play, id, &mut r, &meta, ctx.as_ref())?
                }
            };
            // Isolated packets (`misc.rs`).
            let misc = match (&own, phase) {
                (Handled::No, Phase::Configuration) => crate::misc::config(&dv, id, &mut r)?,
                (Handled::No, Phase::Play) => crate::misc::play(&dv, id, &mut r)?,
                _ => None,
            };
            if let Some(e) = misc {
                if let Some(e) = e {
                    emit(next, e);
                }
                continue;
            }
            // Progress and statistics (`progress.rs`).
            if matches!((&own, phase), (Handled::No, Phase::Play))
                && let Some(e) = crate::progress::play(&pg, id, &mut r, &items)?
            {
                emit(next, e);
                continue;
            }
            match own {
                Handled::No => {}
                Handled::Event(e) => {
                    emit(next, e);
                    continue;
                }
                Handled::Transfer(e) => {
                    // Nothing is applied after the transfer: the connection is closed and the
                    // game opens another (cookies carried over).
                    next.bundle = None;
                    emit(next, e);
                    link.shutdown();
                    return Ok("transfer".into());
                }
                Handled::Done => continue,
            }
            match phase {
                Phase::Login => {
                    if id == ids.login_compression {
                        let t = read_varint(&mut r)?;
                        link.set_compression(t);
                        dec.set_compression_unvalidated(t);
                    } else if id == ids.login_finished {
                        emit(next, Event::LoginFinished);
                        link.send(ids.login_acknowledged, |_| {})?;
                        phase = Phase::Configuration;
                        link.names.set_phase(rc_data::Phase::Configuration);
                        // Client brand, before the client information (§ 13): that of the official game,
                        // Rustcraft means to be indistinguishable.
                        link.send(ids.cfg_custom_payload_out, |w| {
                            rc_proto::write_str(w, "minecraft:brand");
                            rc_proto::write_str(w, "vanilla");
                        })?;
                        let info = cfg.info.clone();
                        link.send(ids.cfg_client_information, |w| play::write_client_information(w, &info))?;
                    } else if id == ids.login_encryption_request {
                        // Online server (`authentication.md` § 2): `join`, then plaintext reply
                        // and encryption in both directions.
                        let req = crate::session::read_encryption_request(&mut r)?;
                        emit(next, Event::LoginStatus("connect.authorizing"));
                        match crate::session::answer(&req, cfg.token.as_ref(), uuid) {
                            Ok((body, secret)) => {
                                emit(next, Event::LoginStatus("connect.encrypting"));
                                link.send(ids.login_key, |w| w.extend_from_slice(&body))?;
                                link.enable_encryption(&secret);
                                dec.enable_encryption(&secret);
                            }
                            Err(reason) => {
                                emit(next, Event::Kicked(*reason));
                                link.shutdown();
                                return Ok("account login failure".into());
                            }
                        }
                    } else if id == ids.login_custom_query {
                        // Login plugin request: the game understands no channel and
                        // always replies "no response" (payload absent), state "negotiating".
                        let transaction = read_varint(&mut r)?;
                        let _channel = rc_proto::read_str(&mut r, 32767)?;
                        if r.remaining() > 1 << 20 {
                            return Err(invalid("Payload may not be larger than 1048576 bytes"));
                        }
                        emit(next, Event::LoginStatus("connect.negotiating"));
                        link.send(ids.login_custom_query_answer, |w| {
                            write_varint(w, transaction);
                            w.push(0);
                        })?;
                    } else if id == ids.login_disconnect {
                        // Reason as JSON during login.
                        let json = rc_proto::read_str(&mut r, 262_144).unwrap_or("?");
                        let reason = serde_json::from_str::<serde_json::Value>(json)
                            .ok()
                            .and_then(|v| crate::status::json_component(&v));
                        if let Some(c) = reason {
                            emit(next, Event::Kicked(c));
                        }
                        return Ok(format!("refused: {json}"));
                    }
                }
                Phase::Configuration => {
                    if id == ids.cfg_select_known_packs {
                        // Like the official client: the base pack is announced if it matches
                        // our data (the server then no longer sends the registry content,
                        // 9.7 KiB instead of 154 KiB). Otherwise empty list: full content.
                        let offered = read_known_packs(&mut r)?;
                        let known: Vec<_> = offered
                            .into_iter()
                            .filter(|(ns, id, v)| {
                                ns == "minecraft"
                                    && id == "core"
                                    && *v == data.version
                                    && !data.dimension_types.is_empty()
                            })
                            .collect();
                        link.send(ids.cfg_select_known_packs_out, |w| {
                            write_varint(w, known.len() as i32);
                            for (ns, id, v) in &known {
                                rc_proto::write_str(w, ns);
                                rc_proto::write_str(w, id);
                                rc_proto::write_str(w, v);
                            }
                        })?;
                    } else if id == ids.cfg_registry_data {
                        let reg = read_registry_data(&mut r)?;
                        match reg.registry.as_str() {
                            "minecraft:dimension_type" => {
                                dim_names = reg.entries.iter().map(|(n, _)| n.clone()).collect();
                                dims = reg
                                    .entries
                                    .iter()
                                    .map(|(name, n)| match n {
                                        Some(n) => DimensionHeight::from_nbt(n),
                                        // Content omitted (known pack): height read from our data.
                                        None => data.dimension_types.iter().find(|t| &t.name == name).map(|t| {
                                            DimensionHeight {
                                                min_y: t.min_y,
                                                height: t.height,
                                                nether_light: t.nether_light,
                                            }
                                        }),
                                    })
                                    .collect();
                            }
                            "minecraft:worldgen/biome" => {
                                biome_names = reg.entries.iter().map(|(n, _)| n.clone()).collect();
                            }
                            "minecraft:world_clock" => {
                                clock_names = reg.entries.iter().map(|(n, _)| n.clone()).collect();
                            }
                            "minecraft:chat_type" => {
                                chat_types = reg
                                    .entries
                                    .iter()
                                    .map(|(n, nbt)| crate::chat::chat_type(n, nbt.as_ref()))
                                    .collect();
                            }
                            // Animal variants: order of the network identifiers and optional content.
                            // Animal variants; enchantments (tooltips, click fingerprints).
                            // Dialogs (`show_dialog` by identifier).
                            name if name.ends_with("_variant")
                                || name == "minecraft:enchantment"
                                || name == "minecraft:dialog"
                                || name == "minecraft:banner_pattern"
                                || name == "minecraft:trim_material"
                                || name == "minecraft:trim_pattern"
                                || name == "minecraft:instrument"
                                || name == "minecraft:jukebox_song"
                                || name == "minecraft:damage_type"
                                || name == "minecraft:test_instance" =>
                            {
                                emit(next, Event::Registry { name: name.to_owned(), entries: reg.entries });
                            }
                            _ => {}
                        }
                    } else if id == ids.cfg_keep_alive {
                        let v = i64::decode(&mut r)?;
                        link.send(ids.cfg_keep_alive_out, |w| v.encode(w))?;
                    } else if id == ids.cfg_ping {
                        let v = i32::decode(&mut r)?;
                        link.send(ids.cfg_pong, |w| v.encode(w))?;
                    } else if id == ids.cfg_code_of_conduct {
                        // Player's reply (screen) or acceptance already remembered: application side,
                        // `Link::send_accept_code_of_conduct` (the server waits).
                        emit(next, Event::CodeOfConduct(rc_proto::read_str(&mut r, 32767)?.to_owned()));
                    } else if id == ids.cfg_finish {
                        link.send(ids.cfg_finish_out, |_| {})?;
                        phase = Phase::Play;
                        link.names.set_phase(rc_data::Phase::Play);
                    } else if id == ids.cfg_update_tags {
                        emit(next, Event::Tags(play::read_update_tags(&mut r)?));
                    } else if id == ids.cfg_custom_payload {
                        link.stats.custom_payload(&mut r)?;
                    } else if id == ids.cfg_disconnect {
                        if let Some(c) = rc_proto::nbt::read(&mut r).ok().flatten().as_ref().and_then(rc_text::from_nbt)
                        {
                            emit(next, Event::Kicked(c));
                        }
                        return Ok("disconnected during configuration".into());
                    }
                }
                Phase::Play => {
                    if id == ids.login || id == ids.respawn {
                        let mut keep = 0u8;
                        let (dim_type, flat, joined, last_death) = if id == ids.login {
                            let l = read_login(&mut r)?;
                            link.set_player_id(l.player_id);
                            // New play session: chat reset; chat keys requested
                            // only on an online server (`authentication.md` § 3.2).
                            chat_index = 0;
                            remote = crate::chat_verify::Remote::new(
                                l.enforces_secure_chat,
                                crate::chat_verify::services_keys(),
                            );
                            let generation = link.chat_reset(uuid, l.online_mode, l.enforces_secure_chat);
                            if l.online_mode
                                && let Some(t) = &cfg.token
                            {
                                crate::signing::spawn_key_task(Arc::downgrade(link), t.clone(), generation);
                            }
                            (l.spawn.dimension_type, l.spawn.flat, Some(l), None)
                        } else {
                            let s = read_spawn_info(&mut r)?;
                            keep = r.u8()?;
                            let death = s.last_death.map(|(d, p)| (d, [p.x, p.y, p.z]));
                            (s.dimension_type, s.flat, None, death)
                        };
                        let dimension = dim_names.get(dim_type as usize).cloned().unwrap_or_default();
                        let height = dims
                            .get(dim_type as usize)
                            .copied()
                            .flatten()
                            .ok_or_else(|| invalid(format!("unknown dimension type {dim_type}")))?;
                        ctx = Some(ChunkContext::new(height.sections(), states, biome_names.len() as u32));
                        match joined {
                            Some(login) => emit(
                                next,
                                Event::Joined {
                                    login: Box::new(login),
                                    height,
                                    biomes: biome_names.clone(),
                                    dimension,
                                    flat,
                                },
                            ),
                            None => emit(next, Event::Respawn { height, dimension, flat, keep, last_death }),
                        }
                    } else if id == ids.chunk {
                        let c = ctx.ok_or_else(|| invalid("chunk before entering the game"))?;
                        match decode_heavy(
                            &packet,
                            (ids.chunk, ids.light_update, ids.commands, ids.map_item_data, ids.recipe_book_add),
                            &c,
                            &items,
                        ) {
                            Event::Disconnected(reason) => return Ok(reason),
                            ev => emit(next, ev),
                        }
                    } else if id == ids.chunk_batch_start {
                        batch_start = Instant::now();
                    } else if id == ids.chunk_batch_finished {
                        let size = read_varint(&mut r)?;
                        rate.on_batch(size, batch_start.elapsed().as_nanos() as f64);
                        let desired = rate.desired_chunks_per_tick();
                        link.send(ids.chunk_batch_received, |w| desired.encode(w))?;
                    } else if id == ids.forget_chunk {
                        let (x, z) = play::read_chunk_pos(&mut r)?;
                        emit(next, Event::ForgetChunk { x, z });
                    } else if id == ids.keep_alive {
                        let v = i64::decode(&mut r)?;
                        link.send(ids.keep_alive_out, |w| v.encode(w))?;
                    } else if id == ids.ping {
                        let v = i32::decode(&mut r)?;
                        link.send(ids.pong, |w| v.encode(w))?;
                    } else if id == ids.player_position {
                        emit(next, Event::Teleport(play::read_player_position(&mut r)?));
                    } else if id == ids.chunk_center {
                        let x = read_varint(&mut r)?;
                        let z = read_varint(&mut r)?;
                        emit(next, Event::ChunkCenter { x, z });
                    } else if id == ids.chunk_radius {
                        emit(next, Event::ViewDistance(read_varint(&mut r)?));
                    } else if id == ids.block_update {
                        let (pos, state) = play::read_block_update(&mut r)?;
                        check_state(state, states)?;
                        emit(next, Event::BlockUpdate { pos, state });
                    } else if id == ids.section_update {
                        let u = play::read_section_update(&mut r)?;
                        for &(_, state) in &u.changes {
                            check_state(state, states)?;
                        }
                        emit(next, Event::SectionUpdate { section: u.section, changes: u.changes });
                    } else if id == ids.block_event {
                        let (pos, b0, b1, block) = play::read_block_event(&mut r)?;
                        emit(next, Event::BlockEvent { pos: [pos.x, pos.y, pos.z], b0, b1, block });
                    } else if id == ids.block_entity_data {
                        let (pos, kind, nbt) = play::read_block_entity_data(&mut r)?;
                        emit(next, Event::BlockEntityData { pos: [pos.x, pos.y, pos.z], kind, nbt });
                    } else if id == ids.start_configuration {
                        link.leave_play()?;
                        link.send(ids.configuration_acknowledged, |_| {})?;
                        link.stats.reset_server();
                        phase = Phase::Configuration;
                        link.names.set_phase(rc_data::Phase::Configuration);
                    } else if id == ids.update_tags {
                        emit(next, Event::Tags(play::read_update_tags(&mut r)?));
                    } else if id == ids.update_attributes {
                        let (entity, attributes) = play::read_update_attributes(&mut r)?;
                        emit(next, Event::Attributes { entity, attributes });
                    } else if id == ids.set_time {
                        let (game_time, clocks) = play::read_set_time(&mut r)?;
                        let clocks = clocks
                            .into_iter()
                            .map(|(c, t, p, s)| (clock_names.get(c as usize).cloned().unwrap_or_default(), t, p, s))
                            .collect();
                        emit(next, Event::Time { game_time, clocks });
                    } else if id == ids.system_chat {
                        let (content, overlay) = play::read_system_chat(&mut r)?;
                        let c = content
                            .as_ref()
                            .and_then(rc_text::from_nbt)
                            .unwrap_or_else(|| rc_text::Component::text(""));
                        if overlay {
                            emit(next, Event::ActionBar(c));
                        } else if crate::chat::visibility() != 2 && crate::chat::system_message_shown(&c) {
                            // Chat hidden: even system messages are ignored; guessed sender
                            // blocked or non-friend: message hidden.
                            emit(next, Event::Chat { component: c, tag: crate::ChatTag::System });
                        }
                    } else if id == ids.action_bar {
                        let c = rc_proto::nbt::read(&mut r)?.as_ref().and_then(rc_text::from_nbt);
                        emit(next, Event::ActionBar(c.unwrap_or_else(|| rc_text::Component::text(""))));
                    } else if id == ids.player_chat {
                        let m = play::read_player_chat(&mut r)?;
                        if m.global_index != chat_index {
                            return Ok(crate::reason::key("multiplayer.disconnect.bad_chat_index"));
                        }
                        chat_index += 1;
                        // Signature cache and last seen (`chat.md` § 1.7, § 2.4).
                        let last_seen = match link.chat_unpack(&m) {
                            Ok(l) => l,
                            Err(e) => return Ok(e),
                        };
                        // Validation (sender's chat session), then display (§ 1.8).
                        let now = crate::signing::now_ms();
                        let verdict = remote.validate(&m, &last_seen, now);
                        // Launcher and account restrictions: multiplayer server only.
                        let visible = crate::chat::visibility() == 0 && crate::session::player_chat_allowed();
                        let (line, shown) = crate::chat::player_message(&m, &chat_types, verdict, visible, now);
                        // Signature kept by the displayed message only if it is signed and
                        // verified; last seen tracked for any message carrying a signature
                        // except the accepted unsigned one (signature removed).
                        let kept = matches!(verdict, crate::chat_verify::Verdict::Accepted { signed: true });
                        if let Some(s) = &m.signature
                            && verdict != (crate::chat_verify::Verdict::Accepted { signed: false })
                            && let Err(e) = link.chat_processed(s, shown && kept)
                        {
                            return Ok(e);
                        }
                        if let Some(l) = line {
                            emit(next, Event::Chat { component: l.component, tag: l.tag });
                            if let Some(o) = l.original {
                                emit(next, Event::ChatOriginal(o));
                            }
                            if kept && let Some(s) = m.signature.clone() {
                                emit(next, Event::ChatSignature(s));
                            }
                        }
                    } else if id == ids.delete_chat {
                        let packed = match read_varint(&mut r)? {
                            0 => Ok(r.take_array()?),
                            v => Err(v as u32 - 1),
                        };
                        match link.chat_deleted(packed) {
                            Ok(s) => emit(next, Event::DeleteChat(Box::new(s))),
                            Err(e) => return Ok(e),
                        }
                    } else if id == ids.commands {
                        match commands_event(&mut r) {
                            Event::Disconnected(reason) => return Ok(reason),
                            ev => {
                                if let Event::Commands(t) = &ev {
                                    link.set_commands(t);
                                }
                                emit(next, ev)
                            }
                        }
                    } else if id == ids.command_suggestions {
                        emit(
                            next,
                            Event::CommandSuggestions(rc_proto_v777::commands::read_command_suggestions(&mut r)?),
                        );
                    } else if id == ids.custom_chat_completions {
                        let (action, entries) = rc_proto_v777::commands::read_chat_completions(&mut r)?;
                        emit(next, Event::ChatCompletions { action, entries });
                    } else if id == ids.disguised_chat {
                        let (msg, kind, sender, target) = play::read_disguised_chat(&mut r)?;
                        let content =
                            msg.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| rc_text::Component::text(""));
                        let component =
                            crate::chat::decorate(&kind, &chat_types, content, sender.as_ref(), target.as_ref());
                        // Like a player message: absent if player chat is off.
                        if crate::chat::visibility() == 0 && crate::session::player_chat_allowed() {
                            emit(next, Event::Chat { component, tag: crate::ChatTag::Disguised });
                        }
                    } else if id == ids.light_update {
                        let c = ctx.ok_or_else(|| invalid("light before entering the game"))?;
                        let x = read_varint(&mut r)?;
                        let z = read_varint(&mut r)?;
                        let light = rc_proto_v777::chunk::read_light(&mut r, c.sections)?;
                        emit(next, Event::LightUpdate { x, z, light: Box::new(light) });
                    } else if id == ids.game_event {
                        let (kind, param) = play::read_game_event(&mut r)?;
                        emit(next, Event::GameEvent { kind, param });
                    } else if let Some(read) = border_reader(&ids, id) {
                        emit(next, Event::Border(read(&mut r)?));
                    } else if id == ids.player_info_update {
                        let u = rc_proto_v777::players::read_info_update(&mut r)?;
                        remote.apply(&u).map_err(invalid)?;
                        for e in &u.entries {
                            if let Some((name, _)) = &e.profile {
                                crate::chat::discover(name, e.uuid);
                            }
                        }
                        emit(next, Event::PlayerInfo(u));
                    } else if id == ids.set_objective {
                        emit(next, Event::Score(rc_proto_v777::scores::read_set_objective(&mut r)?));
                    } else if id == ids.set_display_objective {
                        emit(next, Event::Score(rc_proto_v777::scores::read_display_objective(&mut r)?));
                    } else if id == ids.set_score {
                        emit(next, Event::Score(rc_proto_v777::scores::read_set_score(&mut r)?));
                    } else if id == ids.reset_score {
                        emit(next, Event::Score(rc_proto_v777::scores::read_reset_score(&mut r)?));
                    } else if id == ids.set_player_team {
                        emit(next, Event::Score(rc_proto_v777::scores::read_player_team(&mut r)?));
                    } else if id == ids.sound {
                        emit(next, Event::Sound(play::read_sound(&mut r)?));
                    } else if id == ids.sound_entity {
                        emit(next, Event::Sound(play::read_sound_entity(&mut r)?));
                    } else if id == ids.level_particles {
                        emit(next, Event::Particles(meta.read_level_particles(&mut r)?));
                    } else if id == ids.block_changed_ack {
                        emit(next, Event::BlockChangedAck(read_varint(&mut r)?));
                    } else if id == ids.block_destruction {
                        let (id, pos, stage) = rc_proto_v777::interact::read_block_destruction(&mut r)?;
                        emit(next, Event::BlockDestruction { id, pos, stage });
                    } else if id == ids.level_event {
                        let (kind, pos, data, global) = play::read_level_event(&mut r)?;
                        emit(next, Event::LevelEvent { kind, pos: [pos.x, pos.y, pos.z], data, global });
                    } else if id == ids.stop_sound {
                        let (category, name) = play::read_stop_sound(&mut r)?;
                        emit(next, Event::StopSound { category, name });
                    } else if id == ids.boss_event {
                        emit(next, Event::Boss(play::read_boss_event(&mut r)?));
                    } else if id == ids.waypoint {
                        emit(next, Event::Waypoint(rc_proto_v777::waypoint::read_waypoint(&mut r)?));
                    } else if id == ids.map_item_data {
                        emit(next, Event::MapData(rc_proto_v777::map::read_map_item_data(&mut r)?));
                    } else if id == ids.set_title_text {
                        emit(next, Event::Title(rc_proto::nbt::read(&mut r)?));
                    } else if id == ids.set_subtitle_text {
                        emit(next, Event::Subtitle(rc_proto::nbt::read(&mut r)?));
                    } else if id == ids.set_titles_animation {
                        emit(next, Event::TitleTimes(play::read_titles_animation(&mut r)?));
                    } else if id == ids.clear_titles {
                        emit(next, Event::ClearTitles(bool::decode(&mut r)?));
                    } else if id == ids.tab_list {
                        let (header, footer) = play::read_tab_list(&mut r)?;
                        emit(next, Event::TabList { header, footer });
                    } else if id == ids.player_info_remove {
                        let gone = rc_proto_v777::players::read_info_remove(&mut r)?;
                        remote.remove(&gone);
                        emit(next, Event::PlayersRemoved(gone));
                    } else if id == ids.cooldown {
                        // Cooldown group (identifier) then duration in ticks.
                        let raw = rc_proto::read_str(&mut r, 32767)?;
                        let group = if raw.contains(':') { raw.to_owned() } else { format!("minecraft:{raw}") };
                        let ticks = read_varint(&mut r)?;
                        emit(next, Event::Cooldown { group, ticks });
                    } else if id == ids.set_health {
                        let (health, food, saturation) = play::read_set_health(&mut r)?;
                        emit(next, Event::Health { health, food, saturation });
                    } else if id == ids.set_experience {
                        let (progress, level, total) = play::read_set_experience(&mut r)?;
                        emit(next, Event::Experience { progress, level, total });
                    } else if id == ids.container_set_content
                        || id == ids.container_set_slot
                        || id == ids.set_player_inventory
                        || id == ids.set_cursor_item
                    {
                        let inv = if id == ids.container_set_content {
                            items.read_content(&mut r)?
                        } else if id == ids.container_set_slot {
                            items.read_slot(&mut r)?
                        } else if id == ids.set_player_inventory {
                            items.read_player_slot(&mut r)?
                        } else {
                            items.read_cursor(&mut r)?
                        };
                        // No byte must remain: a badly read component shows up right away.
                        r.finish()?;
                        emit(next, Event::Inventory(inv));
                    } else if id == ids.recipe_book_add
                        || id == ids.recipe_book_remove
                        || id == ids.recipe_book_settings
                        || id == ids.place_ghost_recipe
                    {
                        let ev = if id == ids.recipe_book_add {
                            crate::recipes::read_add(&mut r, &items)?
                        } else if id == ids.recipe_book_remove {
                            crate::recipes::read_remove(&mut r)?
                        } else if id == ids.recipe_book_settings {
                            crate::recipes::read_settings(&mut r)?
                        } else {
                            crate::recipes::read_ghost(&mut r, &items)?
                        };
                        r.finish()?;
                        emit(next, Event::Recipes(ev));
                    } else if id == ids.open_screen {
                        let (window, kind, title) = rc_proto_v777::container::read_open_screen(&mut r)?;
                        let title = title.as_ref().and_then(rc_text::from_nbt);
                        emit(next, Event::OpenScreen { window, kind, title });
                    } else if id == ids.merchant_offers {
                        let o = rc_proto_v777::special::read_merchant_offers(&items, &mut r)?;
                        emit(next, Event::MerchantOffers(Box::new(o)));
                    } else if id == ids.update_recipes {
                        let rec = rc_proto_v777::special::read_update_recipes(&items, &mut r)?;
                        emit(next, Event::UpdateRecipes(Box::new(rec)));
                    } else if id == ids.mount_screen_open {
                        let (window, columns, entity) = rc_proto_v777::special::read_mount_screen_open(&mut r)?;
                        emit(next, Event::MountScreen { window, columns, entity });
                    } else if id == ids.open_book {
                        emit(next, Event::OpenBook(read_varint(&mut r)?));
                    } else if id == ids.open_sign_editor {
                        let (p, front) = rc_proto_v777::special::read_open_sign_editor(&mut r)?;
                        emit(next, Event::OpenSignEditor { pos: [p.x, p.y, p.z], front });
                    } else if id == ids.tag_query {
                        let transaction = read_varint(&mut r)?;
                        let nbt = rc_proto::nbt::read(&mut r)?;
                        emit(next, Event::TagQuery { id: transaction, nbt });
                    } else if id == ids.test_instance_block_status {
                        let status = rc_proto::nbt::read(&mut r)?.as_ref().and_then(rc_text::from_nbt);
                        let size = if bool::decode(&mut r)? {
                            Some([read_varint(&mut r)?, read_varint(&mut r)?, read_varint(&mut r)?])
                        } else {
                            None
                        };
                        emit(next, Event::TestInstanceStatus { status, size });
                    } else if id == ids.game_test_highlight_pos {
                        emit(next, read_game_test_highlight(&mut r)?);
                    } else if id == ids.container_close {
                        read_varint(&mut r)?;
                        emit(next, Event::ContainerClosed);
                    } else if id == ids.container_set_data {
                        let (window, prop, value) = rc_proto_v777::container::read_set_data(&mut r)?;
                        emit(next, Event::ContainerData { window, prop, value });
                    } else if let Some(ev) = entity_packet(id, &ids, &meta, &mut r)? {
                        emit(next, Event::Entity(ev));
                    } else if id == ids.spawn_position {
                        let (dimension, pos, _, _) = play::read_spawn_position(&mut r)?;
                        emit(next, Event::SpawnPosition { dimension, pos: [pos.x, pos.y, pos.z] });
                    } else if id == ids.player_combat_kill {
                        let entity = read_varint(&mut r)?;
                        let message = rc_proto::nbt::read(&mut r)?.as_ref().and_then(rc_text::from_nbt);
                        emit(next, Event::Died { entity, message });
                    } else if id == ids.set_held_slot {
                        emit(next, Event::HeldSlot(read_varint(&mut r)?));
                    } else if id == ids.abilities {
                        emit(next, Event::Abilities(play::read_abilities(&mut r)?));
                    } else if id == ids.custom_payload {
                        link.stats.custom_payload(&mut r)?;
                    } else if id == ids.pong_response {
                        link.stats.pong(&mut r)?;
                    } else if id == ids.ticking_state {
                        link.stats.ticking_state(&mut r)?;
                    } else if id == ids.ticking_step {
                        link.stats.ticking_step(&mut r)?;
                    } else if id == ids.debug_sample {
                        link.stats.debug_sample(&mut r)?;
                    } else if id == ids.disconnect {
                        let reason = rc_proto::nbt::read(&mut r).ok().flatten();
                        if let Some(c) = reason.as_ref().and_then(rc_text::from_nbt) {
                            emit(next, Event::Kicked(c));
                        }
                        return Ok(format!("disconnected by the server: {reason:?}"));
                    }
                }
            }
        }
    }
}

/// Entity packets (`entities.md` part 1); `None` if `id` is not one.
fn entity_packet(
    id: i32,
    ids: &crate::ids::Ids,
    meta: &rc_proto_v777::entity::MetaCodec,
    r: &mut Reader<'_>,
) -> rc_proto::Result<Option<crate::EntityEvent>> {
    use crate::EntityEvent as E;
    use rc_proto_v777::entity as en;
    Ok(Some(if id == ids.add_entity {
        E::Add(Box::new(en::read_add_entity(r)?))
    } else if id == ids.remove_entities {
        E::Remove(en::read_remove_entities(r)?)
    } else if id == ids.move_entity_pos {
        E::Move(en::read_move_pos(r, false)?)
    } else if id == ids.move_entity_pos_rot {
        E::Move(en::read_move_pos(r, true)?)
    } else if id == ids.move_entity_rot {
        E::Move(en::read_move_rot(r)?)
    } else if id == ids.teleport_entity {
        E::Teleport(en::read_teleport_entity(r)?)
    } else if id == ids.entity_position_sync {
        E::Sync(en::read_position_sync(r)?)
    } else if id == ids.rotate_head {
        let (id, yaw) = en::read_rotate_head(r)?;
        E::Head { id, yaw }
    } else if id == ids.set_entity_motion {
        let (id, velocity) = en::read_motion(r)?;
        E::Motion { id, velocity }
    } else if id == ids.set_entity_data {
        let (id, values) = meta.read_entity_data(r)?;
        E::Data { id, values }
    } else if id == ids.set_equipment {
        let (id, slots) = meta.read_equipment(r)?;
        E::Equipment { id, slots }
    } else if id == ids.entity_event {
        let (id, event) = en::read_entity_event(r)?;
        E::Event { id, event }
    } else if id == ids.swing_animation {
        let (id, hand, kind, duration) = en::read_swing(r)?;
        E::Swing { id, hand, kind, duration }
    } else if id == ids.hurt_animation {
        let (id, yaw) = en::read_hurt_animation(r)?;
        E::Hurt { id, yaw }
    } else if id == ids.damage_event {
        let (id, source) = en::read_damage_event(r)?;
        E::Damage { id, source }
    } else if id == ids.set_passengers {
        let (vehicle, passengers) = en::read_passengers(r)?;
        E::Passengers { vehicle, passengers }
    } else if id == ids.update_mob_effect {
        let (id, effect, amplifier, duration, flags) = en::read_mob_effect(r)?;
        E::Effect { id, effect, amplifier, duration, flags }
    } else if id == ids.remove_mob_effect {
        let (id, effect) = en::read_remove_mob_effect(r)?;
        E::RemoveEffect { id, effect }
    } else if id == ids.animate {
        // `animate`: entity, action (0 wake-up, 1 critical hit, 2 magic hit).
        E::Animate { id: read_varint(r)?, action: u8::decode(r)? }
    } else {
        return Ok(None);
    }))
}

/// Identifiers of the packets of `extra.rs`, per phase.
struct ExtraIds {
    login: crate::extra::Ids3,
    cfg_common: crate::extra::Ids3,
    play_common: crate::extra::Ids3,
    cfg: crate::extra::CfgIds,
    play: crate::extra::PlayIds,
}

fn extra_ids(i: &Ids) -> ExtraIds {
    use crate::extra::{CfgIds, Ids3, PlayIds};
    ExtraIds {
        // The login phase only has cookies.
        login: Ids3 {
            cookie_request: i.login_cookie_request,
            cookie_response: i.login_cookie_response,
            store_cookie: -1,
            pack_push: -1,
            pack_pop: -1,
            pack_out: -1,
        },
        cfg_common: Ids3 {
            cookie_request: i.cfg_cookie_request,
            cookie_response: i.cfg_cookie_response,
            store_cookie: i.cfg_store_cookie,
            pack_push: i.cfg_resource_pack_push,
            pack_pop: i.cfg_resource_pack_pop,
            pack_out: i.cfg_resource_pack_out,
        },
        play_common: Ids3 {
            cookie_request: i.cookie_request,
            cookie_response: i.cookie_response,
            store_cookie: i.store_cookie,
            pack_push: i.resource_pack_push,
            pack_pop: i.resource_pack_pop,
            pack_out: i.resource_pack_out,
        },
        cfg: CfgIds {
            transfer: i.cfg_transfer,
            features: i.cfg_update_enabled_features,
            reset_chat: i.cfg_reset_chat,
            report: i.cfg_custom_report_details,
            links: i.cfg_server_links,
            show_dialog: i.cfg_show_dialog,
            clear_dialog: i.cfg_clear_dialog,
        },
        play: PlayIds {
            ticking_state: i.ticking_state,
            ticking_step: i.ticking_step,
            explode: i.explode,
            rotation: i.player_rotation,
            look_at: i.player_look_at,
            camera: i.set_camera,
            vehicle: i.move_vehicle,
            transfer: i.transfer,
            report: i.custom_report_details,
            links: i.server_links,
            show_dialog: i.show_dialog,
            clear_dialog: i.clear_dialog,
            biomes: i.chunks_biomes,
            simulation: i.set_simulation_distance,
            difficulty: i.change_difficulty,
            game_rules: i.game_rule_values,
        },
    }
}

/// List of the packs offered by the server: (namespace, identifier, version).
type BorderRead = fn(&mut Reader<'_>) -> rc_proto::Result<rc_proto_v777::border::BorderPacket>;

/// Reader of the packet with boundary identifier `id`, if it is one.
fn border_reader(ids: &Ids, id: i32) -> Option<BorderRead> {
    use rc_proto_v777::border as b;
    let table: [(i32, BorderRead); 6] = [
        (ids.initialize_border, b::read_init),
        (ids.set_border_center, b::read_center),
        (ids.set_border_lerp_size, b::read_lerp),
        (ids.set_border_size, b::read_size),
        (ids.set_border_warning_delay, b::read_warning_delay),
        (ids.set_border_warning_distance, b::read_warning_distance),
    ];
    table.iter().find(|t| t.0 == id).map(|t| t.1)
}

fn read_known_packs(r: &mut Reader<'_>) -> rc_proto::Result<Vec<(String, String, String)>> {
    let n = read_varint(r)?;
    if !(0..=64).contains(&n) {
        return Err(rc_proto::Error::LengthOutOfRange { len: n.into(), max: 64 });
    }
    (0..n)
        .map(|_| {
            let mut s = || rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT).map(str::to_owned);
            Ok((s()?, s()?, s()?))
        })
        .collect()
}

/// Unknown state identifier: decoding error that ends the session, like the game
/// (the mesher and the physics index their tables by state).
fn check_state(state: u32, states: u32) -> Result<(), Error> {
    if state >= states {
        return Err(invalid(format!("unknown block state {state}")));
    }
    Ok(())
}

/// `commands` packet (`commands.md` § 1); unreadable tree = decoding error.
/// `RUSTCRAFT_CMDDUMP=<file>`: raw body kept for the decoding tests.
fn commands_event(r: &mut Reader<'_>) -> Event {
    if let Some(path) = rc_data::env_os!("RUSTCRAFT_CMDDUMP") {
        let _ = std::fs::write(path, r.rest());
    }
    match rc_proto_v777::commands::read_commands(r) {
        Ok(t) => Event::Commands(Box::new(t)),
        Err(_) => Event::Disconnected(crate::reason::decode_failed("minecraft:commands")),
    }
}

/// Heavy play packet already decompressed: chunks and light updates give an
/// event (identifiers of these two packets in `ids`).
fn decode_heavy(
    raw: &[u8],
    (chunk_id, light_id, commands_id, map_id, recipe_id): (i32, i32, i32, i32, i32),
    ctx: &ChunkContext,
    items: &rc_proto_v777::item::ItemCodec,
) -> Event {
    let mut r = Reader::new(raw);
    match read_varint(&mut r) {
        // Full recipe book on entering the game (often more than 16 KiB).
        Ok(id) if id == recipe_id => {
            match crate::recipes::read_add(&mut r, items).and_then(|e| r.finish().map(|_| e)) {
                Ok(e) => Event::Recipes(e),
                Err(_) => Event::Disconnected(crate::reason::decode_failed("minecraft:recipe_book_add")),
            }
        }
        Ok(id) if id == chunk_id => match read_chunk(&mut r, ctx) {
            Ok(c) => Event::Chunk(Box::new(c)),
            Err(_) => Event::Disconnected(crate::reason::decode_failed("minecraft:level_chunk_with_light")),
        },
        Ok(id) if id == light_id => {
            let light = (|| -> rc_proto::Result<Event> {
                let x = read_varint(&mut r)?;
                let z = read_varint(&mut r)?;
                let light = rc_proto_v777::chunk::read_light(&mut r, ctx.sections)?;
                Ok(Event::LightUpdate { x, z, light: Box::new(light) })
            })();
            light.unwrap_or_else(|_| Event::Disconnected(crate::reason::decode_failed("minecraft:light_update")))
        }
        // Command tree of an operator (often more than 16 KiB compressed).
        Ok(id) if id == commands_id => commands_event(&mut r),
        // Full map (128 × 128 bytes: more than 16 KiB).
        Ok(id) if id == map_id => rc_proto_v777::map::read_map_item_data(&mut r).map_or_else(
            |_| Event::Disconnected(crate::reason::decode_failed("minecraft:map_item_data")),
            Event::MapData,
        ),
        // Progress (full tree on entering the game, often more than 16 KiB).
        Ok(id) => crate::progress::heavy(id, &mut r, items).unwrap_or(Event::Skipped),
        // Other heavy packets (recipes, tags, inventories…): not used yet.
        _ => Event::Skipped,
    }
}

/// `game_test_highlight_pos` (`/test pos`): absolute position then position relative to the test,
/// two packed positions.
fn read_game_test_highlight(r: &mut Reader<'_>) -> rc_proto::Result<Event> {
    let a = rc_proto::BlockPos::decode(r)?;
    let b = rc_proto::BlockPos::decode(r)?;
    Ok(Event::GameTestHighlight { absolute: [a.x, a.y, a.z], relative: [b.x, b.y, b.z] })
}

#[cfg(test)]
mod highlight_tests {
    use super::*;

    #[test]
    fn game_test_highlight_decodes_two_packed_positions() {
        let mut w = Vec::new();
        rc_proto::BlockPos::new(-1234, -60, 98_765).encode(&mut w);
        rc_proto::BlockPos::new(-2, 0, 3).encode(&mut w);
        let mut r = Reader::new(&w);
        let e = read_game_test_highlight(&mut r).expect("decoded");
        assert!(r.is_empty());
        match e {
            Event::GameTestHighlight { absolute, relative } => {
                assert_eq!(absolute, [-1234, -60, 98_765]);
                assert_eq!(relative, [-2, 0, 3]);
            }
            _ => panic!("wrong event"),
        }
    }
}
