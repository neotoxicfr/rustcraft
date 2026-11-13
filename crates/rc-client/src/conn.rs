//! Network thread loop. Reads the socket, splits the frames, answers service packets
//! and publishes the events in order; heavy play-phase packets go
//! compressed to the pool (decompression + decoding), put back in order by the sequencer.
//! Which handler reads a packet is decided by `dispatch.rs`; this module holds the handlers,
//! one per phase, and in play one per group of packets.

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

use crate::dispatch::{
    BorderPacket, ChatPacket, ConfigurationPacket, ContainerPacket, Dispatch, EntityPacket, HudPacket, LoginPacket,
    PlayPacket, SessionPacket, WorldPacket,
};
use crate::extra::{CommonPacket, CommonReplies, Handled};
use crate::ids::Ids;
use crate::link::Link;
use crate::seq::Sequencer;
use crate::{Config, Error, Event, Session};

/// Decompressed size from which a play packet goes to the pool.
const DEFER_FROM: usize = 16 * 1024;

/// Connection and read timeouts (30 s, like the game).
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
        let mut out = Emit::new(&seq);
        let mut decoding = None;
        let r = run(input, &cfg, &l, &d, &mut out, &mut decoding);
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
        seq.complete(out.n, Event::Disconnected(reason));
    })?;
    Ok(Session { events: rx, data, link })
}

/// Numbering of the published events, and open bundle (`bundle_delimiter`,
/// `protocol-supplements.md` § 1): its events are held then published as one block.
struct Emit<'a> {
    seq: &'a Arc<Sequencer<Event>>,
    n: u64,
    bundle: Option<Vec<Event>>,
    /// Packets received in the open bundle.
    bundled: usize,
}

/// Maximum packets in a bundle.
const BUNDLE_MAX: usize = 4096;

impl<'a> Emit<'a> {
    fn new(seq: &'a Arc<Sequencer<Event>>) -> Self {
        Emit { seq, n: 0, bundle: None, bundled: 0 }
    }

    fn push(&mut self, e: Event) {
        match &mut self.bundle {
            Some(b) => b.push(e),
            None => {
                self.seq.complete(self.n, e);
                self.n += 1;
            }
        }
    }
}

fn invalid(e: impl std::fmt::Display) -> Error {
    Error::Invalid(e.to_string())
}

/// What follows a packet.
#[derive(Debug, PartialEq)]
enum Flow {
    /// Read the next packet.
    Next,
    /// End of the session (disconnection by the server, transfer, refused chat), with its reason.
    End(String),
}

/// Returns the reason for the end of the session (disconnection by the server or closing).
fn run(
    mut input: TcpStream,
    cfg: &Config,
    link: &Arc<Link>,
    data: &VersionData,
    out: &mut Emit<'_>,
    decoding: &mut Option<(rc_data::Phase, i32)>,
) -> Result<String, Error> {
    let use_table = rc_data::opt_switch!("RUSTCRAFT_OPT_CLIENT_PACKET_TABLE");
    let mut conn = Connection::new(cfg, link, data, use_table, crate::chat_verify::services_keys);
    conn.greet()?;
    // Local player of the chat (process-wide), set here rather than in `greet` so that the
    // replay tests leave it alone.
    crate::chat::set_local(conn.uuid);
    let mut dec = FrameDecoder::new();
    // Large blocks: online, an encrypted buffer ≥ 128 KiB is decrypted on the pool.
    let mut buf = vec![0u8; 1024 * 1024];
    let mut packet = Vec::new();
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
            let defer = if conn.can_defer() && out.bundle.is_none() { DEFER_FROM } else { usize::MAX };
            let size = match dec.next_or_deferred(&mut packet, defer)? {
                Next::Pending => break,
                Next::Ready => None,
                Next::Deferred { size } => Some(size),
            };
            link.stats.received.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(size) = size {
                conn.decode_on_pool(std::mem::take(&mut packet), size, out);
                continue;
            }
            if let Flow::End(reason) = conn.handle(&packet, out, &mut dec, decoding)? {
                return Ok(reason);
            }
        }
    }
}

/// State of the network thread for one connection: what the configuration taught (registries,
/// dimension heights, chat types), the chat state and the decoders of the play phase.
struct Connection<'a> {
    cfg: &'a Config,
    link: &'a Arc<Link>,
    dispatch: Dispatch,
    progress: crate::progress::ProgressIds,
    /// Key set of the services for the chat sessions (`chat_verify::services_keys`).
    services_keys: fn() -> Arc<Vec<Vec<u8>>>,
    phase: Phase,
    uuid: u128,
    dims: Vec<Option<DimensionHeight>>,
    dim_names: Vec<String>,
    biome_names: Vec<String>,
    clock_names: Vec<String>,
    chat_types: Vec<rc_proto_v777::play::Decoration>,
    chat_index: i32,
    /// Known players and validation of their signed messages (`chat_verify.rs`).
    remote: crate::chat_verify::Remote,
    items: rc_proto_v777::item::ItemCodec,
    meta: rc_proto_v777::entity::MetaCodec,
    /// Item codec handed to the pool's decodings.
    heavy_items: Arc<rc_proto_v777::item::ItemCodec>,
    ctx: Option<ChunkContext>,
    batch_start: Instant,
    rate: ChunkBatchRate,
    states: u32,
    /// Known packs: the version, and whether our data holds the dimension types.
    version: &'a str,
    dimension_types: &'a [rc_data::DimensionType],
}

impl<'a> Connection<'a> {
    fn new(
        cfg: &'a Config,
        link: &'a Arc<Link>,
        data: &'a VersionData,
        use_table: bool,
        services_keys: fn() -> Arc<Vec<Vec<u8>>>,
    ) -> Self {
        let names = |r: &str| data.registry(r).map(|r| r.entries.clone()).unwrap_or_default();
        let items = rc_proto_v777::item::ItemCodec::new(
            &names("minecraft:data_component_type"),
            &names("minecraft:consume_effect_type"),
        );
        let meta = rc_proto_v777::entity::MetaCodec::new(items.clone(), &names("minecraft:particle_type"));
        Connection {
            cfg,
            link,
            dispatch: Dispatch::new(&link.ids, use_table),
            progress: crate::progress::ids(&link.ids),
            services_keys,
            phase: Phase::Login,
            uuid: cfg.uuid.unwrap_or_else(|| crate::offline_uuid(&cfg.name)),
            dims: Vec::new(),
            dim_names: Vec::new(),
            biome_names: Vec::new(),
            clock_names: Vec::new(),
            chat_types: Vec::new(),
            chat_index: 0,
            remote: crate::chat_verify::Remote::new(false, Arc::new(Vec::new())),
            heavy_items: Arc::new(items.clone()),
            items,
            meta,
            ctx: None,
            batch_start: Instant::now(),
            rate: ChunkBatchRate::default(),
            states: data.states.len() as u32,
            version: &data.version,
            dimension_types: &data.dimension_types,
        }
    }

    /// Handshake then login request.
    fn greet(&self) -> Result<(), Error> {
        let (cfg, link) = (self.cfg, self.link);
        // Intent 3 for a connection opened by a transfer (§ 10).
        let intent = if cfg.transfer { 3 } else { 2 };
        link.send(link.ids.intention, |w| {
            play::write_handshake(w, rc_proto_v777::PROTOCOL, &cfg.host, cfg.port, intent)
        })?;
        link.names.set_phase(rc_data::Phase::Login);
        link.send(link.ids.hello, |w| play::write_hello(w, &cfg.name, self.uuid))?;
        Ok(())
    }

    /// Heavy packets may be decoded on the pool once in play with a world entered.
    fn can_defer(&self) -> bool {
        self.phase == Phase::Play && self.ctx.is_some()
    }

    /// Identifiers of the packets `decode_heavy` reads.
    fn heavy_ids(&self) -> (i32, i32, i32, i32, i32) {
        let ids = &self.link.ids;
        (ids.chunk, ids.light_update, ids.commands, ids.map_item_data, ids.recipe_book_add)
    }

    /// Heavy packet: decompression and decoding on the pool, delivered at its rank.
    fn decode_on_pool(&self, compressed: Vec<u8>, size: usize, out: &mut Emit<'_>) {
        let (s, c, heavy) = (out.n, self.ctx.expect("context"), self.heavy_ids());
        out.n += 1;
        let heavy_items = self.heavy_items.clone();
        let (seq, link) = (out.seq.clone(), self.link.clone());
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
    }

    /// One packet (identifier then body, decompressed), read inline.
    fn handle(
        &mut self,
        packet: &[u8],
        out: &mut Emit<'_>,
        dec: &mut FrameDecoder,
        decoding: &mut Option<(rc_data::Phase, i32)>,
    ) -> Result<Flow, Error> {
        let mut r = Reader::new(packet);
        let id = read_varint(&mut r)?;
        *decoding = Some((self.link.names.phase(), id));
        if crate::protolog::enabled() {
            self.link.names.received(id, packet.len());
        }
        match self.phase {
            Phase::Login => {
                let kind = self.dispatch.login.get(id);
                self.login_packet(kind, id, &mut r, out, dec)
            }
            Phase::Configuration => {
                let kind = self.dispatch.configuration.get(id);
                self.configuration_packet(kind, id, &mut r, out)
            }
            Phase::Play => {
                let kind = self.dispatch.play.get(id);
                self.play_packet(kind, id, packet, &mut r, out)
            }
        }
    }

    /// Replies of the cookies and resource packs, in the current phase.
    fn common_replies(&self) -> CommonReplies {
        let ids = &self.link.ids;
        match self.phase {
            Phase::Login => CommonReplies { cookie_response: ids.login_cookie_response, pack_out: -1 },
            Phase::Configuration => {
                CommonReplies { cookie_response: ids.cfg_cookie_response, pack_out: ids.cfg_resource_pack_out }
            }
            Phase::Play => CommonReplies { cookie_response: ids.cookie_response, pack_out: ids.resource_pack_out },
        }
    }

    /// Packets common to the phases: cookies, resource packs (immediate replies).
    fn common_packet(
        &self,
        packet: CommonPacket,
        id: i32,
        r: &mut Reader<'_>,
        out: &mut Emit<'_>,
    ) -> Result<Flow, Error> {
        if let Handled::Event(e) = crate::extra::common(self.link, &self.common_replies(), packet, id, r)? {
            out.push(e);
        }
        Ok(Flow::Next)
    }

    /// Result of a packet of `extra.rs` (configuration or play).
    fn extra_outcome(&self, handled: Handled, out: &mut Emit<'_>) -> Flow {
        match handled {
            Handled::Done => Flow::Next,
            Handled::Event(e) => {
                out.push(e);
                Flow::Next
            }
            Handled::Transfer(e) => {
                // Nothing is applied after the transfer: the connection is closed and the
                // game opens another (cookies carried over).
                out.bundle = None;
                out.push(e);
                self.link.shutdown();
                Flow::End("transfer".into())
            }
        }
    }

    // ---------- Login ----------

    fn login_packet(
        &mut self,
        packet: LoginPacket,
        id: i32,
        r: &mut Reader<'_>,
        out: &mut Emit<'_>,
        dec: &mut FrameDecoder,
    ) -> Result<Flow, Error> {
        let (cfg, link) = (self.cfg, self.link);
        let ids = &link.ids;
        match packet {
            LoginPacket::Common(c) => return self.common_packet(c, id, r, out),
            LoginPacket::Compression => {
                let t = read_varint(r)?;
                link.set_compression(t);
                dec.set_compression_unvalidated(t);
            }
            LoginPacket::Finished => {
                read_login_finished(r)?;
                out.push(Event::LoginFinished);
                link.send(ids.login_acknowledged, |_| {})?;
                self.phase = Phase::Configuration;
                link.names.set_phase(rc_data::Phase::Configuration);
                // Client brand, before the client information (§ 13): that of the official game,
                // Rustcraft means to be indistinguishable.
                link.send(ids.cfg_custom_payload_out, |w| {
                    rc_proto::write_str(w, "minecraft:brand");
                    rc_proto::write_str(w, "vanilla");
                })?;
                link.send(ids.cfg_client_information, |w| play::write_client_information(w, &cfg.info))?;
            }
            LoginPacket::EncryptionRequest => {
                // Online server (`authentication.md` § 2): `join`, then plaintext reply
                // and encryption in both directions.
                let req = crate::session::read_encryption_request(r)?;
                out.push(Event::LoginStatus("connect.authorizing"));
                match crate::session::answer(&req, cfg.token.as_ref(), self.uuid) {
                    Ok((body, secret)) => {
                        out.push(Event::LoginStatus("connect.encrypting"));
                        link.send(ids.login_key, |w| w.extend_from_slice(&body))?;
                        link.enable_encryption(&secret);
                        dec.enable_encryption(&secret);
                    }
                    Err(reason) => {
                        out.push(Event::Kicked(reason));
                        link.shutdown();
                        return Ok(Flow::End("account login failure".into()));
                    }
                }
            }
            LoginPacket::CustomQuery => {
                // Login plugin request: the game understands no channel and
                // always replies "no response" (payload absent), state "negotiating".
                let transaction = read_varint(r)?;
                let _channel = rc_proto::read_str(r, 32767)?;
                if r.remaining() > 1 << 20 {
                    return Err(invalid("Payload may not be larger than 1048576 bytes"));
                }
                out.push(Event::LoginStatus("connect.negotiating"));
                link.send(ids.login_custom_query_answer, |w| {
                    write_varint(w, transaction);
                    w.push(0);
                })?;
            }
            LoginPacket::Disconnect => {
                // Reason as JSON during login.
                let json = rc_proto::read_str(r, 262_144).unwrap_or("?");
                let reason = serde_json::from_str::<serde_json::Value>(json)
                    .ok()
                    .and_then(|v| crate::status::json_component(&v));
                if let Some(c) = reason {
                    out.push(Event::Kicked(Box::new(c)));
                }
                return Ok(Flow::End(format!("refused: {json}")));
            }
            LoginPacket::Ignored => {}
        }
        Ok(Flow::Next)
    }

    // ---------- Configuration ----------

    fn configuration_packet(
        &mut self,
        packet: ConfigurationPacket,
        id: i32,
        r: &mut Reader<'_>,
        out: &mut Emit<'_>,
    ) -> Result<Flow, Error> {
        use ConfigurationPacket as C;
        let link = self.link;
        let ids = &link.ids;
        match packet {
            C::Common(c) => return self.common_packet(c, id, r, out),
            C::Extra(e) => return Ok(self.extra_outcome(crate::extra::config(e, r)?, out)),
            C::PostEffects => crate::misc::config_post_effects(r)?,
            C::SelectKnownPacks => self.select_known_packs(r)?,
            C::RegistryData => self.registry_data(r, out)?,
            C::KeepAlive => {
                let v = i64::decode(r)?;
                link.send(ids.cfg_keep_alive_out, |w| v.encode(w))?;
            }
            C::Ping => {
                let v = i32::decode(r)?;
                link.send(ids.cfg_pong, |w| v.encode(w))?;
            }
            // Player's reply (screen) or acceptance already remembered: application side,
            // `Link::send_accept_code_of_conduct` (the server waits).
            C::CodeOfConduct => out.push(Event::CodeOfConduct(rc_proto::read_str(r, 32767)?.to_owned())),
            C::Finish => {
                link.send(ids.cfg_finish_out, |_| {})?;
                self.phase = Phase::Play;
                link.names.set_phase(rc_data::Phase::Play);
            }
            C::Tags => out.push(Event::Tags(play::read_update_tags(r)?)),
            C::CustomPayload => link.stats.custom_payload(r)?,
            C::Disconnect => {
                if let Some(c) = rc_proto::nbt::read(r).ok().flatten().as_ref().and_then(rc_text::from_nbt) {
                    out.push(Event::Kicked(Box::new(c)));
                }
                return Ok(Flow::End("disconnected during configuration".into()));
            }
            C::Ignored => {}
        }
        Ok(Flow::Next)
    }

    /// Like the official client: the base pack is announced if it matches our data (the
    /// server then no longer sends the registry content, 9.7 KiB instead of 154 KiB).
    /// Otherwise empty list: full content.
    fn select_known_packs(&self, r: &mut Reader<'_>) -> Result<(), Error> {
        let offered = read_known_packs(r)?;
        let known: Vec<_> = offered
            .into_iter()
            .filter(|(ns, id, v)| {
                ns == "minecraft" && id == "core" && v == self.version && !self.dimension_types.is_empty()
            })
            .collect();
        self.link.send(self.link.ids.cfg_select_known_packs_out, |w| {
            write_varint(w, known.len() as i32);
            for (ns, id, v) in &known {
                rc_proto::write_str(w, ns);
                rc_proto::write_str(w, id);
                rc_proto::write_str(w, v);
            }
        })?;
        Ok(())
    }

    fn registry_data(&mut self, r: &mut Reader<'_>, out: &mut Emit<'_>) -> Result<(), Error> {
        let reg = read_registry_data(r)?;
        match reg.registry.as_str() {
            "minecraft:dimension_type" => {
                self.dim_names = reg.entries.iter().map(|(n, _)| n.clone()).collect();
                let types = self.dimension_types;
                self.dims = reg
                    .entries
                    .iter()
                    .map(|(name, n)| match n {
                        Some(n) => DimensionHeight::from_nbt(n),
                        // Content omitted (known pack): height read from our data.
                        None => types.iter().find(|t| &t.name == name).map(|t| DimensionHeight {
                            min_y: t.min_y,
                            height: t.height,
                            nether_light: t.nether_light,
                        }),
                    })
                    .collect();
            }
            "minecraft:worldgen/biome" => {
                self.biome_names = reg.entries.iter().map(|(n, _)| n.clone()).collect();
            }
            "minecraft:world_clock" => {
                self.clock_names = reg.entries.iter().map(|(n, _)| n.clone()).collect();
            }
            "minecraft:chat_type" => {
                self.chat_types = reg.entries.iter().map(|(n, nbt)| crate::chat::chat_type(n, nbt.as_ref())).collect();
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
                out.push(Event::Registry { name: name.to_owned(), entries: reg.entries });
            }
            _ => {}
        }
        Ok(())
    }

    // ---------- Play ----------

    fn play_packet(
        &mut self,
        packet: PlayPacket,
        id: i32,
        raw: &[u8],
        r: &mut Reader<'_>,
        out: &mut Emit<'_>,
    ) -> Result<Flow, Error> {
        match packet {
            PlayPacket::Common(c) => return self.common_packet(c, id, r, out),
            PlayPacket::BundleDelimiter => {
                self.bundle_delimiter(id, out);
                return Ok(Flow::Next);
            }
            _ => {}
        }
        if out.bundle.is_some() {
            out.bundled += 1;
            if out.bundled > BUNDLE_MAX {
                return Err(invalid(format!("bundle of more than {BUNDLE_MAX} packets")));
            }
            if id == self.link.ids.start_configuration {
                return Err(invalid("phase change in a bundle"));
            }
        }
        match packet {
            PlayPacket::Extra(e) => {
                let handled = crate::extra::play(self.link, e, r, &self.meta, self.ctx.as_ref())?;
                Ok(self.extra_outcome(handled, out))
            }
            PlayPacket::Misc(m) => {
                if let Some(e) = crate::misc::play(m, r)? {
                    out.push(e);
                }
                Ok(Flow::Next)
            }
            PlayPacket::Progress => {
                if let Some(e) = crate::progress::play(&self.progress, id, r, &self.items)? {
                    out.push(e);
                }
                Ok(Flow::Next)
            }
            PlayPacket::Session(s) => self.session_packet(s, r, out),
            PlayPacket::World(w) => self.world_packet(w, raw, r, out),
            PlayPacket::Chat(c) => self.chat_packet(c, r, out),
            PlayPacket::Hud(h) => self.hud_packet(h, r, out).map(|()| Flow::Next),
            PlayPacket::Container(c) => self.container_packet(c, r, out).map(|()| Flow::Next),
            PlayPacket::Entity(e) => {
                out.push(Event::Entity(entity_packet(e, &self.meta, r)?));
                Ok(Flow::Next)
            }
            PlayPacket::Common(_) | PlayPacket::BundleDelimiter | PlayPacket::Ignored => Ok(Flow::Next),
        }
    }

    /// Opening or closing of a bundle (§ 1).
    fn bundle_delimiter(&self, id: i32, out: &mut Emit<'_>) {
        match out.bundle.take() {
            Some(b) => {
                let bundled = out.bundled;
                self.link.names.detail_with(rc_data::Bound::ToClient, id, || format!("bundle of {bundled} packets"));
                out.push(Event::Bundle(b));
            }
            None => (out.bundle, out.bundled) = (Some(Vec::new()), 0),
        }
    }

    fn session_packet(&mut self, packet: SessionPacket, r: &mut Reader<'_>, out: &mut Emit<'_>) -> Result<Flow, Error> {
        let link = self.link;
        let ids = &link.ids;
        match packet {
            SessionPacket::Login => self.enter_world(true, r, out)?,
            SessionPacket::Respawn => self.enter_world(false, r, out)?,
            SessionPacket::KeepAlive => {
                let v = i64::decode(r)?;
                link.send(ids.keep_alive_out, |w| v.encode(w))?;
            }
            SessionPacket::Ping => {
                let v = i32::decode(r)?;
                link.send(ids.pong, |w| v.encode(w))?;
            }
            SessionPacket::StartConfiguration => {
                link.leave_play()?;
                link.send(ids.configuration_acknowledged, |_| {})?;
                link.stats.reset_server();
                self.phase = Phase::Configuration;
                link.names.set_phase(rc_data::Phase::Configuration);
            }
            SessionPacket::Tags => out.push(Event::Tags(play::read_update_tags(r)?)),
            SessionPacket::CustomPayload => link.stats.custom_payload(r)?,
            SessionPacket::PongResponse => link.stats.pong(r)?,
            SessionPacket::DebugSample => link.stats.debug_sample(r)?,
            SessionPacket::Disconnect => {
                let reason = rc_proto::nbt::read(r).ok().flatten();
                if let Some(c) = reason.as_ref().and_then(rc_text::from_nbt) {
                    out.push(Event::Kicked(Box::new(c)));
                }
                return Ok(Flow::End(format!("disconnected by the server: {reason:?}")));
            }
        }
        Ok(Flow::Next)
    }

    /// `login` (`first`) or `respawn`: dimension of the new world and context of its chunks.
    fn enter_world(&mut self, first: bool, r: &mut Reader<'_>, out: &mut Emit<'_>) -> Result<(), Error> {
        let link = self.link;
        let mut keep = 0u8;
        let (dim_type, flat, joined, last_death) = if first {
            let l = read_login(r)?;
            link.set_player_id(l.player_id);
            // New play session: chat reset; chat keys requested
            // only on an online server (`authentication.md` § 3.2).
            self.chat_index = 0;
            self.remote = crate::chat_verify::Remote::new(l.enforces_secure_chat, (self.services_keys)());
            let generation = link.chat_reset(self.uuid, l.online_mode, l.enforces_secure_chat);
            if l.online_mode
                && let Some(t) = &self.cfg.token
            {
                crate::signing::spawn_key_task(Arc::downgrade(link), t.clone(), generation);
            }
            (l.spawn.dimension_type, l.spawn.flat, Some(l), None)
        } else {
            let s = read_spawn_info(r)?;
            keep = r.u8()?;
            let death = s.last_death.map(|(d, p)| (d, [p.x, p.y, p.z]));
            (s.dimension_type, s.flat, None, death)
        };
        let dimension = self.dim_names.get(dim_type as usize).cloned().unwrap_or_default();
        let height = self
            .dims
            .get(dim_type as usize)
            .copied()
            .flatten()
            .ok_or_else(|| invalid(format!("unknown dimension type {dim_type}")))?;
        self.ctx = Some(ChunkContext::new(height.sections(), self.states, self.biome_names.len() as u32));
        out.push(match joined {
            Some(login) => {
                Event::Joined { login: Box::new(login), height, biomes: self.biome_names.clone(), dimension, flat }
            }
            None => Event::Respawn { height, dimension, flat, keep, last_death },
        });
        Ok(())
    }

    fn world_packet(
        &mut self,
        packet: WorldPacket,
        raw: &[u8],
        r: &mut Reader<'_>,
        out: &mut Emit<'_>,
    ) -> Result<Flow, Error> {
        use WorldPacket as W;
        let event = match packet {
            W::Chunk => {
                let c = self.ctx.ok_or_else(|| invalid("chunk before entering the game"))?;
                match decode_heavy(raw, self.heavy_ids(), &c, &self.items) {
                    Event::Disconnected(reason) => return Ok(Flow::End(reason)),
                    ev => ev,
                }
            }
            W::ChunkBatchStart => {
                self.batch_start = Instant::now();
                return Ok(Flow::Next);
            }
            W::ChunkBatchFinished => {
                let size = read_varint(r)?;
                self.rate.on_batch(size, self.batch_start.elapsed().as_nanos() as f64);
                let desired = self.rate.desired_chunks_per_tick();
                self.link.send(self.link.ids.chunk_batch_received, |w| desired.encode(w))?;
                return Ok(Flow::Next);
            }
            W::ForgetChunk => {
                let (x, z) = play::read_chunk_pos(r)?;
                Event::ForgetChunk { x, z }
            }
            W::PlayerPosition => Event::Teleport(play::read_player_position(r)?),
            W::ChunkCenter => {
                let x = read_varint(r)?;
                let z = read_varint(r)?;
                Event::ChunkCenter { x, z }
            }
            W::ChunkRadius => Event::ViewDistance(read_varint(r)?),
            W::BlockUpdate => {
                let (pos, state) = play::read_block_update(r)?;
                check_state(state, self.states)?;
                Event::BlockUpdate { pos, state }
            }
            W::SectionUpdate => {
                let u = play::read_section_update(r)?;
                for &(_, state) in &u.changes {
                    check_state(state, self.states)?;
                }
                Event::SectionUpdate { section: u.section, changes: u.changes }
            }
            W::BlockEvent => {
                let (pos, b0, b1, block) = play::read_block_event(r)?;
                Event::BlockEvent { pos: [pos.x, pos.y, pos.z], b0, b1, block }
            }
            W::BlockEntityData => {
                let (pos, kind, nbt) = play::read_block_entity_data(r)?;
                Event::BlockEntityData { pos: [pos.x, pos.y, pos.z], kind, nbt }
            }
            W::Attributes => {
                let (entity, attributes) = play::read_update_attributes(r)?;
                Event::Attributes { entity, attributes }
            }
            W::Time => {
                let (game_time, clocks) = play::read_set_time(r)?;
                let names = &self.clock_names;
                let clocks = clocks
                    .into_iter()
                    .map(|(c, t, p, s)| (names.get(c as usize).cloned().unwrap_or_default(), t, p, s))
                    .collect();
                Event::Time { game_time, clocks }
            }
            W::LightUpdate => {
                let c = self.ctx.ok_or_else(|| invalid("light before entering the game"))?;
                let x = read_varint(r)?;
                let z = read_varint(r)?;
                let light = rc_proto_v777::chunk::read_light(r, c.sections)?;
                Event::LightUpdate { x, z, light: Box::new(light) }
            }
            W::GameEvent => {
                let (kind, param) = play::read_game_event(r)?;
                Event::GameEvent { kind, param }
            }
            W::Border(b) => Event::Border(read_border(b, r)?),
            W::Sound => Event::Sound(play::read_sound(r)?),
            W::SoundEntity => Event::Sound(play::read_sound_entity(r)?),
            W::LevelParticles => Event::Particles(Box::new(self.meta.read_level_particles(r)?)),
            W::BlockChangedAck => Event::BlockChangedAck(read_varint(r)?),
            W::BlockDestruction => {
                let (id, pos, stage) = rc_proto_v777::interact::read_block_destruction(r)?;
                Event::BlockDestruction { id, pos, stage }
            }
            W::LevelEvent => {
                let (kind, pos, data, global) = play::read_level_event(r)?;
                Event::LevelEvent { kind, pos: [pos.x, pos.y, pos.z], data, global }
            }
            W::StopSound => {
                let (category, name) = play::read_stop_sound(r)?;
                Event::StopSound { category, name }
            }
            W::MapData => Event::MapData(rc_proto_v777::map::read_map_item_data(r)?),
            W::SpawnPosition => {
                let (dimension, pos, _, _) = play::read_spawn_position(r)?;
                Event::SpawnPosition { dimension, pos: [pos.x, pos.y, pos.z] }
            }
        };
        out.push(event);
        Ok(Flow::Next)
    }

    fn chat_packet(&mut self, packet: ChatPacket, r: &mut Reader<'_>, out: &mut Emit<'_>) -> Result<Flow, Error> {
        match packet {
            ChatPacket::SystemChat => {
                let (content, overlay) = play::read_system_chat(r)?;
                let c = content.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| rc_text::Component::text(""));
                if overlay {
                    out.push(Event::ActionBar(Box::new(c)));
                } else if crate::chat::visibility() != 2 && crate::chat::system_message_shown(&c) {
                    // Chat hidden: even system messages are ignored; guessed sender
                    // blocked or non-friend: message hidden.
                    out.push(Event::Chat { component: Box::new(c), tag: crate::ChatTag::System });
                }
            }
            ChatPacket::ActionBar => {
                let c = rc_proto::nbt::read(r)?.as_ref().and_then(rc_text::from_nbt);
                out.push(Event::ActionBar(Box::new(c.unwrap_or_else(|| rc_text::Component::text("")))));
            }
            ChatPacket::PlayerChat => return self.player_chat(r, out),
            ChatPacket::DeleteChat => {
                let packed = match read_varint(r)? {
                    0 => Ok(r.take_array()?),
                    v => Err(v as u32 - 1),
                };
                match self.link.chat_deleted(packed) {
                    Ok(s) => out.push(Event::DeleteChat(Box::new(s))),
                    Err(e) => return Ok(Flow::End(e)),
                }
            }
            ChatPacket::Commands => match commands_event(r) {
                Event::Disconnected(reason) => return Ok(Flow::End(reason)),
                ev => {
                    if let Event::Commands(t) = &ev {
                        self.link.set_commands(t);
                    }
                    out.push(ev)
                }
            },
            ChatPacket::CommandSuggestions => {
                out.push(Event::CommandSuggestions(rc_proto_v777::commands::read_command_suggestions(r)?))
            }
            ChatPacket::ChatCompletions => {
                let (action, entries) = rc_proto_v777::commands::read_chat_completions(r)?;
                out.push(Event::ChatCompletions { action, entries });
            }
            ChatPacket::DisguisedChat => {
                let (msg, kind, sender, target) = play::read_disguised_chat(r)?;
                let content = msg.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| rc_text::Component::text(""));
                let component =
                    crate::chat::decorate(&kind, &self.chat_types, content, sender.as_ref(), target.as_ref());
                // Like a player message: absent if player chat is off.
                if crate::chat::visibility() == 0 && crate::session::player_chat_allowed() {
                    out.push(Event::Chat { component: Box::new(component), tag: crate::ChatTag::Disguised });
                }
            }
        }
        Ok(Flow::Next)
    }

    /// Signed player message (`chat.md` § 1.7 to 1.8).
    fn player_chat(&mut self, r: &mut Reader<'_>, out: &mut Emit<'_>) -> Result<Flow, Error> {
        let link = self.link;
        let m = play::read_player_chat(r)?;
        if m.global_index != self.chat_index {
            return Ok(Flow::End(crate::reason::key("multiplayer.disconnect.bad_chat_index")));
        }
        self.chat_index += 1;
        // Signature cache and last seen (`chat.md` § 1.7, § 2.4).
        let last_seen = match link.chat_unpack(&m) {
            Ok(l) => l,
            Err(e) => return Ok(Flow::End(e)),
        };
        // Validation (sender's chat session), then display (§ 1.8).
        let now = crate::signing::now_ms();
        let verdict = self.remote.validate(&m, &last_seen, now);
        // Launcher and account restrictions: multiplayer server only.
        let visible = crate::chat::visibility() == 0 && crate::session::player_chat_allowed();
        let (line, shown) = crate::chat::player_message(&m, &self.chat_types, verdict, visible, now);
        // Signature kept by the displayed message only if it is signed and
        // verified; last seen tracked for any message carrying a signature
        // except the accepted unsigned one (signature removed).
        let kept = matches!(verdict, crate::chat_verify::Verdict::Accepted { signed: true });
        if let Some(s) = &m.signature
            && verdict != (crate::chat_verify::Verdict::Accepted { signed: false })
            && let Err(e) = link.chat_processed(s, shown && kept)
        {
            return Ok(Flow::End(e));
        }
        if let Some(l) = line {
            out.push(Event::Chat { component: Box::new(l.component), tag: l.tag });
            if let Some(o) = l.original {
                out.push(Event::ChatOriginal(o));
            }
            if kept && let Some(s) = m.signature.clone() {
                out.push(Event::ChatSignature(s));
            }
            // Chat log: shown messages that passed validation (not the error line).
            if m.signature.is_some() && matches!(verdict, crate::chat_verify::Verdict::Accepted { .. }) {
                out.push(Event::ChatSender(m.sender));
            }
        }
        Ok(Flow::Next)
    }

    fn hud_packet(&mut self, packet: HudPacket, r: &mut Reader<'_>, out: &mut Emit<'_>) -> Result<(), Error> {
        use HudPacket as H;
        use rc_proto_v777::scores;
        let event = match packet {
            H::PlayerInfoUpdate => {
                let u = rc_proto_v777::players::read_info_update(r)?;
                self.remote.apply(&u).map_err(invalid)?;
                for e in &u.entries {
                    if let Some((name, _)) = &e.profile {
                        crate::chat::discover(name, e.uuid);
                    }
                }
                Event::PlayerInfo(u)
            }
            H::SetObjective => Event::Score(Box::new(scores::read_set_objective(r)?)),
            H::DisplayObjective => Event::Score(Box::new(scores::read_display_objective(r)?)),
            H::SetScore => Event::Score(Box::new(scores::read_set_score(r)?)),
            H::ResetScore => Event::Score(Box::new(scores::read_reset_score(r)?)),
            H::PlayerTeam => Event::Score(Box::new(scores::read_player_team(r)?)),
            H::BossEvent => Event::Boss(play::read_boss_event(r)?),
            H::Waypoint => Event::Waypoint(Box::new(rc_proto_v777::waypoint::read_waypoint(r)?)),
            H::TitleText => Event::Title(rc_proto::nbt::read(r)?),
            H::SubtitleText => Event::Subtitle(rc_proto::nbt::read(r)?),
            H::TitlesAnimation => Event::TitleTimes(play::read_titles_animation(r)?),
            H::ClearTitles => Event::ClearTitles(bool::decode(r)?),
            H::TabList => {
                let (header, footer) = play::read_tab_list(r)?;
                Event::TabList { header, footer }
            }
            H::PlayerInfoRemove => {
                let gone = rc_proto_v777::players::read_info_remove(r)?;
                self.remote.remove(&gone);
                Event::PlayersRemoved(gone)
            }
            H::Cooldown => {
                // Cooldown group (identifier) then duration in ticks.
                let raw = rc_proto::read_str(r, 32767)?;
                let group = if raw.contains(':') { raw.to_owned() } else { format!("minecraft:{raw}") };
                let ticks = read_varint(r)?;
                Event::Cooldown { group, ticks }
            }
            H::Health => {
                let (health, food, saturation) = play::read_set_health(r)?;
                Event::Health { health, food, saturation }
            }
            H::Experience => {
                let (progress, level, total) = play::read_set_experience(r)?;
                Event::Experience { progress, level, total }
            }
            H::CombatKill => {
                let entity = read_varint(r)?;
                let message = rc_proto::nbt::read(r)?.as_ref().and_then(rc_text::from_nbt).map(Box::new);
                Event::Died { entity, message }
            }
            H::HeldSlot => Event::HeldSlot(read_varint(r)?),
            H::Abilities => Event::Abilities(play::read_abilities(r)?),
        };
        out.push(event);
        Ok(())
    }

    fn container_packet(&self, packet: ContainerPacket, r: &mut Reader<'_>, out: &mut Emit<'_>) -> Result<(), Error> {
        use ContainerPacket as K;
        let items = &self.items;
        let event = match packet {
            K::SetContent | K::SetSlot | K::SetPlayerInventory | K::SetCursorItem => {
                let inv = match packet {
                    K::SetContent => items.read_content(r)?,
                    K::SetSlot => items.read_slot(r)?,
                    K::SetPlayerInventory => items.read_player_slot(r)?,
                    _ => items.read_cursor(r)?,
                };
                // No byte must remain: a badly read component shows up right away.
                r.finish()?;
                Event::Inventory(inv)
            }
            K::RecipeBookAdd | K::RecipeBookRemove | K::RecipeBookSettings | K::PlaceGhostRecipe => {
                let ev = match packet {
                    K::RecipeBookAdd => crate::recipes::read_add(r, items)?,
                    K::RecipeBookRemove => crate::recipes::read_remove(r)?,
                    K::RecipeBookSettings => crate::recipes::read_settings(r)?,
                    _ => crate::recipes::read_ghost(r, items)?,
                };
                r.finish()?;
                Event::Recipes(ev)
            }
            K::OpenScreen => {
                let (window, kind, title) = rc_proto_v777::container::read_open_screen(r)?;
                let title = title.as_ref().and_then(rc_text::from_nbt).map(Box::new);
                Event::OpenScreen { window, kind, title }
            }
            K::MerchantOffers => {
                Event::MerchantOffers(Box::new(rc_proto_v777::special::read_merchant_offers(items, r)?))
            }
            K::UpdateRecipes => Event::UpdateRecipes(Box::new(rc_proto_v777::special::read_update_recipes(items, r)?)),
            K::MountScreenOpen => {
                let (window, columns, entity) = rc_proto_v777::special::read_mount_screen_open(r)?;
                Event::MountScreen { window, columns, entity }
            }
            K::OpenBook => Event::OpenBook(read_varint(r)?),
            K::OpenSignEditor => {
                let (p, front) = rc_proto_v777::special::read_open_sign_editor(r)?;
                Event::OpenSignEditor { pos: [p.x, p.y, p.z], front }
            }
            K::TagQuery => {
                let transaction = read_varint(r)?;
                let nbt = rc_proto::nbt::read(r)?;
                Event::TagQuery { id: transaction, nbt }
            }
            K::TestInstanceStatus => {
                let status = rc_proto::nbt::read(r)?.as_ref().and_then(rc_text::from_nbt).map(Box::new);
                let size =
                    if bool::decode(r)? { Some([read_varint(r)?, read_varint(r)?, read_varint(r)?]) } else { None };
                Event::TestInstanceStatus { status, size }
            }
            K::GameTestHighlight => read_game_test_highlight(r)?,
            K::Close => {
                read_varint(r)?;
                Event::ContainerClosed
            }
            K::SetData => {
                let (window, prop, value) = rc_proto_v777::container::read_set_data(r)?;
                Event::ContainerData { window, prop, value }
            }
        };
        out.push(event);
        Ok(())
    }
}

/// Entity packets (`entities.md` part 1).
fn entity_packet(
    packet: EntityPacket,
    meta: &rc_proto_v777::entity::MetaCodec,
    r: &mut Reader<'_>,
) -> rc_proto::Result<crate::EntityEvent> {
    use crate::EntityEvent as E;
    use rc_proto_v777::entity as en;
    Ok(match packet {
        EntityPacket::Add => E::Add(Box::new(en::read_add_entity(r)?)),
        EntityPacket::Remove => E::Remove(en::read_remove_entities(r)?),
        EntityPacket::MovePos => E::Move(en::read_move_pos(r, false)?),
        EntityPacket::MovePosRot => E::Move(en::read_move_pos(r, true)?),
        EntityPacket::MoveRot => E::Move(en::read_move_rot(r)?),
        EntityPacket::Teleport => E::Teleport(en::read_teleport_entity(r)?),
        EntityPacket::PositionSync => E::Sync(en::read_position_sync(r)?),
        EntityPacket::RotateHead => {
            let (id, yaw) = en::read_rotate_head(r)?;
            E::Head { id, yaw }
        }
        EntityPacket::Motion => {
            let (id, velocity) = en::read_motion(r)?;
            E::Motion { id, velocity }
        }
        EntityPacket::Data => {
            let (id, values) = meta.read_entity_data(r)?;
            E::Data { id, values }
        }
        EntityPacket::Equipment => {
            let (id, slots) = meta.read_equipment(r)?;
            E::Equipment { id, slots }
        }
        EntityPacket::Event => {
            let (id, event) = en::read_entity_event(r)?;
            E::Event { id, event }
        }
        EntityPacket::Swing => {
            let (id, hand, kind, duration) = en::read_swing(r)?;
            E::Swing { id, hand, kind, duration }
        }
        EntityPacket::Hurt => {
            let (id, yaw) = en::read_hurt_animation(r)?;
            E::Hurt { id, yaw }
        }
        EntityPacket::Damage => {
            let (id, source) = en::read_damage_event(r)?;
            E::Damage { id, source }
        }
        EntityPacket::Passengers => {
            let (vehicle, passengers) = en::read_passengers(r)?;
            E::Passengers { vehicle, passengers }
        }
        EntityPacket::Effect => {
            let (id, effect, amplifier, duration, flags) = en::read_mob_effect(r)?;
            E::Effect { id, effect, amplifier, duration, flags }
        }
        EntityPacket::RemoveEffect => {
            let (id, effect) = en::read_remove_mob_effect(r)?;
            E::RemoveEffect { id, effect }
        }
        // `animate`: entity, action (0 wake-up, 1 critical hit, 2 magic hit).
        EntityPacket::Animate => E::Animate { id: read_varint(r)?, action: u8::decode(r)? },
    })
}

/// One of the six world border packets.
fn read_border(packet: BorderPacket, r: &mut Reader<'_>) -> rc_proto::Result<rc_proto_v777::border::BorderPacket> {
    use rc_proto_v777::border as b;
    match packet {
        BorderPacket::Initialize => b::read_init(r),
        BorderPacket::Center => b::read_center(r),
        BorderPacket::LerpSize => b::read_lerp(r),
        BorderPacket::Size => b::read_size(r),
        BorderPacket::WarningDelay => b::read_warning_delay(r),
        BorderPacket::WarningDistance => b::read_warning_distance(r),
    }
}

/// List of the packs offered by the server: (namespace, identifier, version).
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

/// The login's last packet, read like the game's client: the profile (UUID, name, at most 16
/// properties), then the session identifier, and nothing after. A packet without the identifier
/// is a decoding failure that ends the connection, as in the game.
fn read_login_finished(r: &mut Reader<'_>) -> rc_proto::Result<()> {
    let _profile_id = u128::decode(r)?;
    let _name = rc_proto::read_str(r, 16)?;
    let properties = rc_proto::read_len(r, 16)?;
    for _ in 0..properties {
        let _name = rc_proto::read_str(r, 64)?;
        let _value = rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT)?;
        if bool::decode(r)? {
            let _signature = rc_proto::read_str(r, 1024)?;
        }
    }
    let _session_id = u128::decode(r)?;
    r.finish()
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

    /// The login's last packet must hold the session identifier after the profile, and nothing
    /// more: a server that stops after the profile (or writes extra bytes) is refused like the
    /// game's client refuses it.
    #[test]
    fn login_finished_needs_the_session_identifier() {
        let mut profile = Vec::new();
        7u128.encode(&mut profile);
        rc_proto::write_str(&mut profile, "Agent");
        write_varint(&mut profile, 0);
        assert!(read_login_finished(&mut Reader::new(&profile)).is_err(), "profile alone accepted");
        let mut full = profile.clone();
        9u128.encode(&mut full);
        assert!(read_login_finished(&mut Reader::new(&full)).is_ok(), "profile and session refused");
        full.push(0);
        assert!(read_login_finished(&mut Reader::new(&full)).is_err(), "extra byte accepted");
    }

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

#[cfg(test)]
#[path = "conn_replay_tests.rs"]
mod replay_tests;
