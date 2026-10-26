//! LAN TCP connections (S9): handshake, status (server list), authenticated and encrypted login
//! like the game's integrated server (`auth.rs`), compression (threshold
//! 256), configuration (known packs, registries, tags), then play: one thread reads the
//! client's packets, another writes the server's.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crossbeam_channel::{Receiver, Sender};
use rc_client::ids::Ids;
use rc_data::{Bound, Phase, VersionData};
use rc_net::{FrameDecoder, FrameEncoder};
use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{Decode, Encode, Reader, read_str, write_str};

/// Compression threshold of the integrated server.
pub const COMPRESSION: i32 = 256;

/// What the connection threads know about the open world.
pub struct Shared {
    pub data: Arc<VersionData>,
    pub ids: Ids,
    pub motd: String,
    pub max_players: usize,
    /// Connected players (host included): count and names (status sample), in order of
    /// arrival, with their choice to appear in server lists.
    pub online: AtomicUsize,
    pub names: std::sync::Mutex<Vec<(String, u128, bool)>>,
    pub synced: Vec<(String, Vec<String>)>,
    pub tags: rc_proto_v777::play::Tags,
    pub stop: AtomicBool,
    /// World icon (`data:image/png;base64,…`, 64 × 64) for the status.
    pub icon: Option<String>,
    /// RSA key pair, created by the listening thread before the first connection.
    pub keys: std::sync::OnceLock<super::auth::Keys>,
    /// Host profile (name, UUID).
    pub host: (String, u128),
    /// Last status built and its time.
    pub status: std::sync::Mutex<Option<(std::time::Instant, String)>>,
    /// Enabled features of the world (`enabled_features`), sent at configuration.
    pub features: Vec<String>,
}

/// Player arrived in game, passed to the server.
pub struct Joined {
    pub name: String,
    pub uuid: u128,
    /// Properties of the verified profile (signed skin); empty offline.
    pub properties: Vec<rc_proto_v777::players::Property>,
    pub info: rc_proto_v777::play::ClientInformation,
    /// Server packets (identifier + body) to the client.
    pub out: Sender<Vec<u8>>,
    /// Client packets (identifier + body); closed on disconnection.
    pub inp: Receiver<Vec<u8>>,
}

fn pid(data: &VersionData, phase: Phase, bound: Bound, name: &str) -> i32 {
    data.packet_id(phase, bound, &format!("minecraft:{name}")).unwrap_or(-1)
}

/// Listens on `port` (all interfaces, like the game); one thread per connection.
pub fn listen(port: u16, sh: Arc<Shared>, joined: Sender<Joined>) -> std::io::Result<u16> {
    let l = TcpListener::bind((super::announce::bind_addr(), port))?;
    let port = l.local_addr()?.port();
    l.set_nonblocking(true)?;
    std::thread::Builder::new().name("lan-listen".into()).spawn(move || {
        // Keys of the open world (the game creates them when the integrated server starts);
        // connections wait in the listen queue meanwhile.
        if sh.keys.get().is_none() {
            match super::auth::Keys::generate() {
                Ok(k) => {
                    let _ = sh.keys.set(k);
                }
                Err(e) => eprintln!("lan: {e} (offline guests)"),
            }
        }
        while !sh.stop.load(Ordering::Acquire) {
            match l.accept() {
                Ok((s, addr)) => {
                    let (sh, joined) = (sh.clone(), joined.clone());
                    let _ = std::thread::Builder::new().name("lan-connection".into()).spawn(move || {
                        if let Err(e) = handle(s, &sh, &joined) {
                            eprintln!("lan: {addr}: {e}");
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    eprintln!("lan: listen: {e}");
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
    })?;
    Ok(port)
}

struct Io {
    s: TcpStream,
    dec: FrameDecoder,
    enc: FrameEncoder,
    buf: Vec<u8>,
}

impl Io {
    fn send(&mut self, packet: &[u8]) -> std::io::Result<()> {
        let mut out = Vec::with_capacity(packet.len() + 8);
        self.enc.encode(packet, &mut out).map_err(std::io::Error::other)?;
        self.s.write_all(&out)
    }

    fn send_with(&mut self, id: i32, f: impl FnOnce(&mut Vec<u8>)) -> std::io::Result<()> {
        let mut w = Vec::new();
        write_varint(&mut w, id);
        f(&mut w);
        self.send(&w)
    }

    /// Next packet (blocking).
    fn next(&mut self) -> std::io::Result<Vec<u8>> {
        let mut p = Vec::new();
        loop {
            if self.dec.next_packet(&mut p).map_err(std::io::Error::other)? {
                return Ok(p);
            }
            let n = self.s.read(&mut self.buf)?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "connection closed"));
            }
            self.dec.push(&self.buf[..n]);
        }
    }
}

fn handle(s: TcpStream, sh: &Shared, joined: &Sender<Joined>) -> std::io::Result<()> {
    s.set_nodelay(true)?;
    s.set_nonblocking(false)?; // Windows: the accepted socket inherits the listener's mode.
    s.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
    let mut io = Io { s, dec: FrameDecoder::new(), enc: FrameEncoder::new(), buf: vec![0u8; 64 * 1024] };
    let d = &sh.data;
    // Handshake: protocol, address, port, intent.
    let p = io.next()?;
    let mut r = Reader::new(&p);
    let _id = read_varint(&mut r).map_err(std::io::Error::other)?;
    let protocol = read_varint(&mut r).map_err(std::io::Error::other)?;
    let _host = read_str(&mut r, 255).map_err(std::io::Error::other)?;
    let _port = u16::decode(&mut r).map_err(std::io::Error::other)?;
    let intent = read_varint(&mut r).map_err(std::io::Error::other)?;
    if intent == 1 {
        return status(io, sh);
    }
    let ids = sh.ids;
    // Login: `hello` (name, proposed UUID).
    let p = io.next()?;
    let mut r = Reader::new(&p);
    let _ = read_varint(&mut r);
    let name = read_str(&mut r, 16).map_err(std::io::Error::other)?.to_owned();
    if protocol != rc_proto_v777::PROTOCOL {
        let key = if protocol < rc_proto_v777::PROTOCOL {
            "multiplayer.disconnect.outdated_client"
        } else {
            "multiplayer.disconnect.incompatible"
        };
        let msg = serde_json::json!({ "translate": key, "with": ["26.3"] }).to_string();
        io.send_with(ids.login_disconnect, |w| write_str(w, &msg))?;
        return Ok(());
    }
    // Profile: the host's if it is their name (case-insensitive), otherwise encrypted
    // authentication (the integrated server requires authentication).
    let profile = if name.eq_ignore_ascii_case(&sh.host.0) {
        super::auth::Profile { name: sh.host.0.clone(), uuid: sh.host.1, properties: Vec::new() }
    } else if let Some(keys) = sh.keys.get() {
        let verify = super::auth::VERIFY.load(Ordering::Relaxed);
        let challenge = super::auth::challenge();
        io.send_with(ids.login_encryption_request, |w| {
            super::auth::write_request(w, &keys.public_der, &challenge, verify)
        })?;
        let p = io.next()?;
        let mut r = Reader::new(&p);
        let key = (read_varint(&mut r).ok() == Some(ids.login_key)).then(|| super::auth::read_key(&mut r)).flatten();
        let Some((secret, digest)) = key.and_then(|(s, c)| super::auth::accept_key(keys, &challenge, &s, &c)) else {
            return Err(std::io::Error::other("protocol error (encryption response)"));
        };
        io.enc.enable_encryption(&secret);
        io.dec.enable_encryption(&secret);
        let found = if verify { super::auth::has_joined(&name, &digest) } else { None };
        if found.is_none() && verify {
            eprintln!("lan: {name} not verified by the session services, offline entry (singleplayer)");
        }
        super::auth::verified_or_offline(&name, found)
    } else {
        super::auth::offline(&name)
    };
    let (name, uuid) = (profile.name.clone(), profile.uuid);
    // The host's profile is already in game: the game would send it back (and close the world); we
    // refuse the guest instead.
    if uuid == sh.host.1 {
        let msg = serde_json::json!({ "translate": "multiplayer.disconnect.name_taken" }).to_string();
        io.send_with(ids.login_disconnect, |w| write_str(w, &msg))?;
        return Ok(());
    }
    if sh.online.load(Ordering::Acquire) >= sh.max_players {
        let msg = serde_json::json!({ "translate": "multiplayer.disconnect.server_full" }).to_string();
        io.send_with(ids.login_disconnect, |w| write_str(w, &msg))?;
        return Ok(());
    }
    io.send_with(ids.login_compression, |w| write_varint(w, COMPRESSION))?;
    io.enc.set_compression(COMPRESSION);
    io.dec.set_compression(COMPRESSION);
    io.send_with(ids.login_finished, |w| {
        uuid.encode(w);
        write_str(w, &name);
        super::encode::properties(w, &profile.properties);
    })?;
    // Login acknowledgement, then configuration.
    loop {
        let p = io.next()?;
        let mut r = Reader::new(&p);
        if read_varint(&mut r).ok() == Some(ids.login_acknowledged) {
            break;
        }
    }
    let cfg_brand = pid(d, Phase::Configuration, Bound::ToClient, "custom_payload");
    io.send_with(cfg_brand, |w| {
        write_str(w, "minecraft:brand");
        write_str(w, "vanilla");
    })?;
    let features = pid(d, Phase::Configuration, Bound::ToClient, "update_enabled_features");
    io.send_with(features, |w| write_features(w, &sh.features))?;
    io.send_with(ids.cfg_select_known_packs, |w| {
        write_varint(w, 1);
        write_str(w, "minecraft");
        write_str(w, "core");
        write_str(w, &d.version);
    })?;
    let mut info = rc_proto_v777::play::ClientInformation::default();
    let mut known_core = false;
    loop {
        let p = io.next()?;
        let mut r = Reader::new(&p);
        let Ok(id) = read_varint(&mut r) else { continue };
        if id == ids.cfg_client_information {
            if let Some(i) = read_info(&mut r) {
                info = i;
            }
        } else if id == ids.cfg_select_known_packs_out {
            let n = read_varint(&mut r).unwrap_or(0);
            for _ in 0..n.clamp(0, 64) {
                let ns = read_str(&mut r, 32767).unwrap_or("").to_owned();
                let id = read_str(&mut r, 32767).unwrap_or("").to_owned();
                let v = read_str(&mut r, 32767).unwrap_or("").to_owned();
                if ns == "minecraft" && id == "core" && v == d.version {
                    known_core = true;
                }
            }
            break;
        }
    }
    if !known_core {
        let msg = rc_text::Component::text(
            "Rustcraft: base pack 'core' unknown to the client (registry contents not supported)",
        );
        let n = crate::commands::text::to_nbt(&msg);
        io.send_with(ids.cfg_disconnect, |w| rc_proto::nbt::write(w, Some(&n)))?;
        return Ok(());
    }
    for (reg, entries) in &sh.synced {
        io.send_with(ids.cfg_registry_data, |w| {
            write_str(w, reg);
            write_varint(w, entries.len() as i32);
            for e in entries {
                write_str(w, e);
                false.encode(w);
            }
        })?;
    }
    io.send_with(ids.cfg_update_tags, |w| super::encode::tags(w, &sh.tags))?;
    io.send_with(ids.cfg_finish, |_| {})?;
    loop {
        let p = io.next()?;
        let mut r = Reader::new(&p);
        let Ok(id) = read_varint(&mut r) else { continue };
        if id == ids.cfg_client_information {
            if let Some(i) = read_info(&mut r) {
                info = i;
            }
        } else if id == ids.cfg_finish_out {
            break;
        }
    }
    // Play: reading and writing on two threads.
    io.s.set_read_timeout(None)?;
    let (otx, orx) = crossbeam_channel::unbounded::<Vec<u8>>();
    let (itx, irx) = crossbeam_channel::unbounded::<Vec<u8>>();
    let mut ws = io.s.try_clone()?;
    let mut enc = std::mem::take(&mut io.enc);
    std::thread::Builder::new().name("lan-write".into()).spawn(move || {
        let mut out = Vec::new();
        for p in orx {
            out.clear();
            if enc.encode(&p, &mut out).is_err() || ws.write_all(&out).is_err() {
                break;
            }
        }
        let _ = ws.shutdown(std::net::Shutdown::Both);
    })?;
    joined
        .send(Joined { name: name.clone(), uuid, properties: profile.properties, info, out: otx, inp: irx })
        .map_err(|_| std::io::Error::other("server stopped"))?;
    while let Ok(p) = io.next() {
        if itx.send(p).is_err() {
            break;
        }
    }
    Ok(())
}

/// Body of `update_enabled_features`: the enabled features of the world (experiments included).
pub(crate) fn write_features(w: &mut Vec<u8>, features: &[String]) {
    write_varint(w, features.len() as i32);
    for f in features {
        write_str(w, f);
    }
}

fn read_info(r: &mut Reader<'_>) -> Option<rc_proto_v777::play::ClientInformation> {
    Some(rc_proto_v777::play::ClientInformation {
        language: read_str(r, 16).ok()?.to_owned(),
        view_distance: u8::decode(r).ok()?,
        chat_visibility: read_varint(r).ok()?,
        chat_colors: bool::decode(r).ok()?,
        skin_parts: u8::decode(r).ok()?,
        main_hand: read_varint(r).ok()?,
        text_filtering: bool::decode(r).ok()?,
        allow_listing: bool::decode(r).ok()?,
        particles: read_varint(r).ok()?,
    })
}

/// Simple generator (xorshift) for the status sample.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    /// Integer in `[0, n)` (`n` ≥ 1).
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

fn seed_rng() -> Rng {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    Rng(u64::from_le_bytes(b) | 1)
}

/// Status JSON like the game: description as plain text, players (at most 12 from a randomly
/// drawn offset, shuffled, "Anonymous Player" for anyone who refuses to be listed;
/// empty sample omitted), version, world icon if it exists.
pub(crate) fn status_json(sh: &Shared, version: &str, names: &[(String, u128, bool)], rng: &mut Rng) -> String {
    let n = names.len().min(12);
    let offset = rng.below(names.len() - n + 1);
    let mut sample: Vec<serde_json::Value> = names[offset..offset + n]
        .iter()
        .map(|(name, uuid, listed)| {
            if *listed {
                serde_json::json!({ "id": crate::nbtio::uuid_text(*uuid), "name": name })
            } else {
                serde_json::json!({ "id": "00000000-0000-0000-0000-000000000000", "name": "Anonymous Player" })
            }
        })
        .collect();
    // Game shuffle: from the end to the start, swap with a place drawn before.
    for i in (2..=sample.len()).rev() {
        let j = rng.below(i);
        sample.swap(i - 1, j);
    }
    let mut players = serde_json::Map::new();
    players.insert("max".into(), sh.max_players.into());
    players.insert("online".into(), sh.online.load(Ordering::Acquire).into());
    if !sample.is_empty() {
        players.insert("sample".into(), sample.into());
    }
    // Fields in the game's order (description, players, version, favicon); those of the inner objects
    // are already in the alphabetical order of serde_json.
    let version = serde_json::json!({ "name": version, "protocol": rc_proto_v777::PROTOCOL });
    let icon =
        sh.icon.as_ref().map(|i| format!(",\"favicon\":{}", serde_json::Value::from(i.as_str()))).unwrap_or_default();
    format!(
        "{{\"description\":{},\"players\":{},\"version\":{version}{icon}}}",
        serde_json::Value::from(sh.motd.as_str()),
        serde_json::Value::Object(players)
    )
}

/// World icon for the status: `icon.png` 64 × 64 from the world folder, as a `data:` address.
pub(crate) fn world_icon(world: &std::path::Path) -> Option<String> {
    use base64::Engine;
    let png = std::fs::read(world.join("icon.png")).ok()?;
    // PNG header: signature then IHDR (big-endian width and height).
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        eprintln!("lan: world icon unreadable");
        return None;
    }
    let (w, h) = (u32::from_be_bytes(png[16..20].try_into().ok()?), u32::from_be_bytes(png[20..24].try_into().ok()?));
    if (w, h) != (64, 64) {
        eprintln!("lan: world icon of {w} × {h} instead of 64 × 64");
        return None;
    }
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png)))
}

/// Status (server list): JSON response then ping echo.
fn status(mut io: Io, sh: &Shared) -> std::io::Result<()> {
    let d = &sh.data;
    let resp = pid(d, Phase::Status, Bound::ToClient, "status_response");
    let pong = pid(d, Phase::Status, Bound::ToClient, "pong_response");
    let req = pid(d, Phase::Status, Bound::ToServer, "status_request");
    let ping = pid(d, Phase::Status, Bound::ToServer, "ping_request");
    loop {
        let p = io.next()?;
        let mut r = Reader::new(&p);
        let id = read_varint(&mut r).map_err(std::io::Error::other)?;
        if id == req {
            // The game only redoes its status every 5 s.
            let json = {
                let mut cache = sh.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                match &*cache {
                    Some((t, j)) if t.elapsed() < std::time::Duration::from_secs(5) => j.clone(),
                    _ => {
                        let names = sh.names.lock().map(|n| n.clone()).unwrap_or_default();
                        let j = status_json(sh, &d.version, &names, &mut seed_rng());
                        *cache = Some((std::time::Instant::now(), j.clone()));
                        j
                    }
                }
            };
            io.send_with(resp, |w| write_str(w, &json))?;
        } else if id == ping {
            let v = i64::decode(&mut r).map_err(std::io::Error::other)?;
            io.send_with(pong, |w| v.encode(w))?;
            return Ok(());
        }
    }
}
