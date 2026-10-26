//! Status query of a server (the server list's "ping") and address resolution,
//! `docs/specs/title-screen-servers.md` § 3. Synchronous functions with a timeout,
//! meant to run on a separate thread (the screen starts at most 5 at a time).

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use base64::Engine;
use rc_proto::varint::write_varint;
use rc_proto::{Encode, Reader, read_str};
use serde_json::Value;

use crate::dns;

/// Default port; only this port triggers the SRV lookup.
pub const DEFAULT_PORT: u16 = 25565;
/// Maximum size of an accepted status frame (the JSON response is ≤ 32767 characters).
const MAX_FRAME: usize = 1 << 21;
/// Maximum length of the JSON string, in UTF-16 units.
const MAX_JSON: usize = 32767;
/// Maximum side of an accepted icon (PNG header), in pixels.
const MAX_ICON_SIDE: u32 = 1024;
/// Side required to display an icon.
pub const ICON_SIDE: u32 = 64;

/// Typed address, parsed: host (without brackets for IPv6) and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    pub host: String,
    pub port: u16,
}

impl ServerAddress {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        ServerAddress { host: host.into(), port }
    }

    /// Fallback address for an invalid input.
    pub fn invalid() -> Self {
        ServerAddress::new("server.invalid", DEFAULT_PORT)
    }

    /// Parses `host`, `host:port`, `[v6]` or `[v6]:port`; `None` if empty or malformed.
    /// More than one ":" without brackets: the whole string is the host (bare IPv6, default port).
    pub fn try_parse(input: &str) -> Option<Self> {
        let (host, port) = if let Some(rest) = input.strip_prefix('[') {
            let close = rest.find(']')?;
            let host = &rest[..close];
            let after = &rest[close + 1..];
            let port = match after.strip_prefix(':') {
                Some(p) => Some(p),
                None if after.is_empty() => None,
                None => return None,
            };
            (host, port)
        } else {
            match input.find(':') {
                Some(i) if input[i + 1..].find(':').is_none() => (&input[..i], Some(&input[i + 1..])),
                _ => (input, None),
            }
        };
        let port = match port.filter(|p| !p.is_empty()) {
            None => DEFAULT_PORT,
            Some(p) if p.bytes().all(|c| c.is_ascii_digit()) => p.parse::<u16>().ok()?,
            Some(_) => return None,
        };
        (!host.is_empty()).then(|| ServerAddress::new(host, port))
    }

    /// Tolerant parsing: invalid input → `server.invalid:25565`.
    pub fn parse(input: &str) -> Self {
        Self::try_parse(input).unwrap_or_else(Self::invalid)
    }

    /// Usable address (enables the "join" / "done" buttons).
    pub fn is_valid(input: &str) -> bool {
        Self::try_parse(input).is_some()
    }
}

/// Resolved address: host name and port to announce in the connection handshake
/// (SRV target if any, otherwise the typed address) and candidate socket addresses.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub host: String,
    pub port: u16,
    pub addrs: Vec<SocketAddr>,
}

fn system_resolve(host: &str, port: u16) -> Option<Resolved> {
    let addrs: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        game_pick((host, port).to_socket_addrs().ok()?.collect())
    };
    (!addrs.is_empty()).then(|| Resolved { host: host.to_owned(), port, addrs })
}

/// The game keeps only one address from the system resolution: the first IPv4 if there is one
/// (Java's default preference), otherwise the first. That one is tested against the blocked
/// servers list, then pinged or joined, with no fallback to the others.
fn game_pick(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    addrs.iter().find(|a| a.is_ipv4()).or_else(|| addrs.first()).copied().into_iter().collect()
}

/// Resolution § 3.1: system, then `_minecraft._tcp.<host>` SRV redirect if the port is
/// 25565 (first record; its target replaces the system result). `None`: unknown host,
/// or server on the blocked servers list (same refusal, like the game).
pub fn resolve(addr: &ServerAddress) -> Option<Resolved> {
    resolve_checked(addr, &|h| is_blocked(blocklist(), h))
}

fn resolve_checked(addr: &ServerAddress, blocked: &dyn Fn(&str) -> bool) -> Option<Resolved> {
    let allowed = |r: &Resolved| resolved_allowed(r, blocked);
    // Typed name tested before resolution (same refusal, no needless DNS request).
    if blocked(&addr.host) {
        return None;
    }
    let direct = system_resolve(&addr.host, addr.port);
    if direct.as_ref().is_some_and(|r| !allowed(r)) {
        return None;
    }
    if addr.port == DEFAULT_PORT
        && addr.host.parse::<IpAddr>().is_err()
        && let Some(srv) = dns::lookup_srv(&format!("_minecraft._tcp.{}", addr.host))
    {
        return system_resolve(&srv.target, srv.port).filter(allowed);
    }
    direct
}

/// The game tests the name and the IP of the retained address (only one, see `game_pick`); the SRV
/// target is tested the same way. (Name of a typed IP: the game would do a reverse lookup, not
/// reproduced; the IP itself is tested.)
fn resolved_allowed(r: &Resolved, blocked: &dyn Fn(&str) -> bool) -> bool {
    !blocked(&r.host) && r.addrs.iter().all(|a| !blocked(&java_ip(a.ip())))
}

// --- Blocked servers ---

/// List empty from the start, without a network request: for the tests of the other packets that go through
/// `connect` (no effect if the list has already been read).
pub fn skip_blocked_servers() {
    let _ = BLOCKLIST.set(Default::default());
}

static BLOCKLIST: std::sync::OnceLock<hashbrown::HashSet<String>> = std::sync::OnceLock::new();

/// List published by the Mojang services: one SHA-1 fingerprint (lowercase hexadecimal) per
/// line, of a name, of a `*.domain` pattern or `a.b.c.*`.
const BLOCKLIST_URL: &str = "https://sessionserver.mojang.com/blockedservers";

/// List read once, at first use (like the game); unreadable: no blocked server.
/// Ping threads that arrive during the read wait for it.
fn blocklist() -> &'static hashbrown::HashSet<String> {
    BLOCKLIST.get_or_init(|| {
        if cfg!(test) {
            return Default::default();
        }
        let agent = crate::session::http_agent(Some(Duration::from_secs(5)), true);
        let body = agent.get(BLOCKLIST_URL).call().and_then(|mut r| r.body_mut().read_to_vec());
        match body {
            // Read as ISO-8859-1: one byte per character.
            Ok(b) => b.iter().map(|&c| c as char).collect::<String>().lines().map(str::to_owned).collect(),
            Err(e) => {
                eprintln!("blocked servers list unavailable: {e}");
                Default::default()
            }
        }
    })
}

/// Fingerprint of a candidate name: lowercase, encoded as ISO-8859-1 (out of range: one "?" per
/// character, UTF-16 surrogate pair included), SHA-1 in lowercase hexadecimal.
fn blocklist_hash(name: &str) -> String {
    let mut bytes = Vec::with_capacity(name.len());
    for c in name.to_lowercase().chars() {
        bytes.push(u8::try_from(u32::from(c)).unwrap_or(b'?'));
    }
    let d = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &bytes);
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Blocked name or IP: SRV prefix and trailing dots removed, exact name, then `*.name` and the
/// generic suffixes (`*.b.c`, `*.c`); for an IPv4: `a.b.c.*`, `a.b.*`, `a.*`.
fn is_blocked(list: &hashbrown::HashSet<String>, server: &str) -> bool {
    if list.is_empty() {
        return false;
    }
    let s = server.strip_prefix("_minecraft._tcp.").unwrap_or(server).trim_end_matches('.');
    if s.is_empty() {
        return false;
    }
    let hit = |n: &str| list.contains(&blocklist_hash(n));
    if hit(s) {
        return true;
    }
    let mut parts: Vec<&str> = s.split('.').collect();
    let ip = parts.len() == 4 && parts.iter().all(|p| p.parse::<i32>().is_ok_and(|v| (0..=255).contains(&v)));
    if !ip && hit(&format!("*.{s}")) {
        return true;
    }
    while parts.len() > 1 {
        if ip {
            parts.pop();
        } else {
            parts.remove(0);
        }
        let starred = if ip { format!("{}.*", parts.join(".")) } else { format!("*.{}", parts.join(".")) };
        if hit(&starred) {
            return true;
        }
    }
    false
}

/// IP written like Java: dotted IPv4, IPv6 in 8 unabridged hexadecimal groups.
fn java_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v) => v.to_string(),
        IpAddr::V6(v) => v.segments().iter().map(|s| format!("{s:x}")).collect::<Vec<_>>().join(":"),
    }
}

// --- JSON response ---

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub name: String,
    pub protocol: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplePlayer {
    pub id: u128,
    pub name: String,
}

impl SamplePlayer {
    /// Player anonymised by the server (nil UUID and name "Anonymous Player").
    pub fn is_anonymous(&self) -> bool {
        self.id == 0 && self.name == "Anonymous Player"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Players {
    pub max: i32,
    pub online: i32,
    pub sample: Vec<SamplePlayer>,
}

/// Received icon: raw PNG and dimensions read from its header (≤ 1024 × 1024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favicon {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Favicon {
    /// Checks the PNG header (dimensions ≤ 1024); `None` if unreadable or too large.
    pub fn from_png(png: Vec<u8>) -> Option<Self> {
        let (width, height) = png_size(&png)?;
        (width <= MAX_ICON_SIDE && height <= MAX_ICON_SIDE).then_some(Favicon { png, width, height })
    }

    /// Display requires 64 × 64.
    pub fn is_displayable(&self) -> bool {
        self.width == ICON_SIDE && self.height == ICON_SIDE
    }

    /// 8-bit RGBA pixels (64 × 64 × 4), if the icon is displayable and decodable.
    pub fn rgba(&self) -> Option<Vec<u8>> {
        if !self.is_displayable() {
            return None;
        }
        let mut dec = png::Decoder::new(std::io::Cursor::new(&self.png[..]));
        dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = dec.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        let px = (info.width * info.height) as usize;
        let data = &buf[..info.buffer_size()];
        Some(match info.color_type {
            png::ColorType::Rgba => data.to_vec(),
            png::ColorType::Rgb => data.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
            png::ColorType::GrayscaleAlpha => {
                data.as_chunks::<2>().0.iter().flat_map(|c| [c[0], c[0], c[0], c[1]]).collect()
            }
            png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return None,
        })
        .filter(|v: &Vec<u8>| v.len() == px * 4)
    }

    /// Form stored in `servers.dat` (standard base64, no prefix).
    pub fn to_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.png)
    }
}

/// Width and height read from the IHDR header of a PNG.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if png.len() < 24 || png[..8] != SIG || &png[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Content of the status response; each invalid field is treated as absent.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatus {
    /// Raw JSON text component (string, object or array); `Null` if absent.
    pub description: Value,
    pub players: Option<Players>,
    pub version: Option<Version>,
    pub favicon: Option<Favicon>,
    pub enforces_secure_chat: bool,
}

fn as_int(v: &Value) -> Option<i32> {
    v.as_i64().map(|x| x as i32).or_else(|| v.as_f64().map(|x| x as i32))
}

/// Text UUID with dashes (8-4-4-4-12).
pub fn parse_uuid(s: &str) -> Option<u128> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 || parts.iter().zip([8, 4, 4, 4, 12]).any(|(p, n)| p.is_empty() || p.len() > n) {
        return None;
    }
    // Like Java's reader: each group is a hexadecimal number of length ≤ expected.
    let mut v: u128 = 0;
    for (p, n) in parts.iter().zip([8u32, 4, 4, 4, 12]) {
        v = (v << (4 * n)) | u64::from_str_radix(p, 16).ok()? as u128;
    }
    Some(v)
}

fn parse_players(v: &Value) -> Option<Players> {
    let max = as_int(v.get("max")?)?;
    let online = as_int(v.get("online")?)?;
    let sample = match v.get("sample") {
        None => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|p| {
                Some(SamplePlayer {
                    id: parse_uuid(p.get("id")?.as_str()?)?,
                    name: p.get("name")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<Vec<_>>>()?,
        Some(_) => return None,
    };
    Some(Players { max, online, sample })
}

fn parse_favicon(v: &Value) -> Option<Favicon> {
    let b64 = v.as_str()?.strip_prefix("data:image/png;base64,")?.replace('\n', "");
    Favicon::from_png(base64::engine::general_purpose::STANDARD.decode(b64).ok()?)
}

/// Tolerant reading of the JSON response (§ 3.3). Unreadable JSON ⇒ everything absent.
pub fn parse_status_json(json: &str) -> ServerStatus {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let description = match v.get("description") {
        Some(d @ (Value::String(_) | Value::Object(_) | Value::Array(_))) => d.clone(),
        _ => Value::Null,
    };
    let version = v.get("version").and_then(|ver| {
        Some(Version { name: ver.get("name")?.as_str()?.to_owned(), protocol: as_int(ver.get("protocol")?)? })
    });
    ServerStatus {
        description,
        players: v.get("players").and_then(parse_players),
        version,
        favicon: v.get("favicon").and_then(parse_favicon),
        enforces_secure_chat: v.get("enforcesSecureChat").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Text component from its JSON (string, object or array), read through its NBT form;
/// depth bounded to 16 (the rest is discarded). `None` if malformed.
pub fn json_component(v: &Value) -> Option<rc_text::Component> {
    use rc_proto::nbt::Nbt;
    fn conv(v: &Value, depth: usize) -> Option<Nbt> {
        if depth > 16 {
            return None;
        }
        Some(match v {
            Value::Null => return None,
            Value::Bool(b) => Nbt::Byte(i8::from(*b)),
            Value::Number(n) => match n.as_i64() {
                Some(i) => i32::try_from(i).map_or(Nbt::Long(i), Nbt::Int),
                None => Nbt::Double(n.as_f64().unwrap_or(0.0)),
            },
            Value::String(s) => Nbt::String(s.clone()),
            Value::Array(a) => Nbt::List(a.iter().filter_map(|x| conv(x, depth + 1)).collect()),
            Value::Object(o) => {
                Nbt::Compound(o.iter().filter_map(|(k, x)| Some((k.clone(), conv(x, depth + 1)?))).collect())
            }
        })
    }
    rc_text::from_nbt(&conv(v, 0)?)
}

/// Plain text of a JSON component (`text`, `extra`, arrays; `translate` rendered by its key).
/// Preview only: styled rendering will go through `rc-text`.
pub fn plain_text(c: &Value) -> String {
    fn walk(c: &Value, out: &mut String, depth: usize) {
        if depth > 16 {
            return;
        }
        match c {
            Value::String(s) => out.push_str(s),
            Value::Array(a) => a.iter().for_each(|x| walk(x, out, depth + 1)),
            Value::Object(o) => {
                if let Some(t) = o.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                } else if let Some(t) = o.get("translate").and_then(Value::as_str) {
                    out.push_str(t);
                }
                if let Some(Value::Array(extra)) = o.get("extra") {
                    extra.iter().for_each(|x| walk(x, out, depth + 1));
                }
            }
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Null => {}
        }
    }
    let mut s = String::new();
    walk(c, &mut s, 0);
    s
}

// --- Exchange ---

#[derive(Debug, thiserror::Error)]
pub enum StatusError {
    #[error("unknown host")]
    UnknownHost,
    #[error("network: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Proto(#[from] rc_proto::Error),
    #[error("{0}")]
    Invalid(&'static str),
}

/// Result of a modern query.
#[derive(Debug, Clone)]
pub struct Pinged {
    pub status: ServerStatus,
    /// Latency measured on the client side between sending the ping and receiving the pong, in ms.
    pub latency_ms: u64,
    /// Announced protocol equal to ours (otherwise "incompatible").
    pub compatible: bool,
}

/// Result of the legacy ping (servers ≤ 1.6), always "incompatible".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyStatus {
    pub protocol: i32,
    pub version: String,
    pub motd: String,
    pub online: i32,
    pub max: i32,
}

#[derive(Debug, Clone)]
pub enum PingOutcome {
    Modern(Pinged),
    Legacy(LegacyStatus),
}

fn open(addrs: &[SocketAddr], deadline: Instant) -> Result<TcpStream, StatusError> {
    let mut last = None;
    for a in addrs {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(a, left) {
            Ok(s) => {
                s.set_nodelay(true)?;
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(StatusError::Io(last.unwrap_or_else(|| std::io::ErrorKind::TimedOut.into())))
}

fn arm(s: &TcpStream, deadline: Instant) -> std::io::Result<()> {
    let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
    s.set_read_timeout(Some(left))?;
    s.set_write_timeout(Some(left))
}

fn send(s: &mut TcpStream, id: i32, body: &[u8]) -> std::io::Result<()> {
    let mut inner = Vec::with_capacity(body.len() + 5);
    write_varint(&mut inner, id);
    inner.extend_from_slice(body);
    let mut frame = Vec::with_capacity(inner.len() + 5);
    write_varint(&mut frame, inner.len() as i32);
    frame.extend_from_slice(&inner);
    s.write_all(&frame)
}

/// Reads a frame (VarInt length, no compression): identifier and body.
fn recv(s: &mut TcpStream) -> Result<(i32, Vec<u8>), StatusError> {
    let mut len: u32 = 0;
    for i in 0..5 {
        let mut b = [0u8];
        s.read_exact(&mut b)?;
        len |= ((b[0] & 0x7f) as u32) << (7 * i);
        if b[0] & 0x80 == 0 {
            break;
        }
        if i == 4 {
            return Err(StatusError::Invalid("frame length"));
        }
    }
    let len = len as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(StatusError::Invalid("frame length"));
    }
    let mut buf = vec![0; len];
    s.read_exact(&mut buf)?;
    let mut r = Reader::new(&buf);
    let id = rc_proto::varint::read_varint(&mut r)?;
    let start = buf.len() - r.remaining();
    Ok((id, buf[start..].to_vec()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64)
}

/// Status exchange § 3.2 on an already resolved address. The handshake announces the **typed**
/// host and port (not the SRV target).
pub fn query_resolved(typed: &ServerAddress, resolved: &Resolved, timeout: Duration) -> Result<Pinged, StatusError> {
    let deadline = Instant::now() + timeout;
    let mut s = open(&resolved.addrs, deadline)?;
    arm(&s, deadline)?;
    let mut hs = Vec::new();
    rc_proto_v777::play::write_handshake(&mut hs, rc_proto_v777::PROTOCOL, &typed.host, typed.port, 1);
    send(&mut s, 0x00, &hs)?;
    send(&mut s, 0x00, &[])?;
    let (id, body) = recv(&mut s)?;
    if id != 0x00 {
        return Err(StatusError::Invalid("status response expected"));
    }
    let mut r = Reader::new(&body);
    let status = parse_status_json(read_str(&mut r, MAX_JSON)?);
    let mut ping = Vec::with_capacity(8);
    now_ms().encode(&mut ping);
    arm(&s, deadline)?;
    let sent = Instant::now();
    send(&mut s, 0x01, &ping)?;
    let (id, _) = recv(&mut s)?;
    let latency_ms = sent.elapsed().as_millis() as u64;
    match id {
        0x01 => {}
        0x00 => return Err(StatusError::Invalid("multiplayer.status.unrequested")),
        _ => return Err(StatusError::Invalid("pong expected")),
    }
    let compatible = status.version.as_ref().is_some_and(|v| v.protocol == rc_proto_v777::PROTOCOL);
    Ok(Pinged { status, latency_ms, compatible })
}

/// Resolves then queries (§ 3.1 + § 3.2).
pub fn query(typed: &ServerAddress, timeout: Duration) -> Result<Pinged, StatusError> {
    let resolved = resolve(typed).ok_or(StatusError::UnknownHost)?;
    query_resolved(typed, &resolved, timeout)
}

fn write_legacy_str(w: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    w.extend_from_slice(&(units.len() as u16).to_be_bytes());
    units.iter().for_each(|u| w.extend_from_slice(&u.to_be_bytes()));
}

/// Legacy ping request (no frame), § 3.5.
pub(crate) fn legacy_request(host: &str, port: u16) -> Vec<u8> {
    let mut w = vec![0xfe, 0x01, 0xfa];
    write_legacy_str(&mut w, "MC|PingHost");
    let mut payload = vec![127];
    write_legacy_str(&mut payload, host);
    payload.extend_from_slice(&(port as i32).to_be_bytes());
    w.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    w.extend_from_slice(&payload);
    w
}

/// Decodes the legacy ping response; `None` if the prefix is not `FF` + `§1`.
pub(crate) fn parse_legacy(b: &[u8]) -> Option<LegacyStatus> {
    if *b.first()? != 0xff {
        return None;
    }
    let n = u16::from_be_bytes([*b.get(1)?, *b.get(2)?]) as usize;
    let units: Vec<u16> = b.get(3..3 + 2 * n)?.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect();
    let s = String::from_utf16_lossy(&units);
    let parts: Vec<&str> = s.splitn(6, '\0').collect();
    if parts.first() != Some(&"§1") || parts.len() < 6 {
        return None;
    }
    let int = |p: &str, d: i32| p.parse::<i32>().unwrap_or(d);
    Some(LegacyStatus {
        protocol: int(parts[1], 0),
        version: parts[2].to_owned(),
        motd: parts[3].to_owned(),
        online: int(parts[4], -1),
        max: int(parts[5], -1),
    })
}

/// Legacy ping on a new connection; typed host and port in the request.
pub fn legacy_query(
    typed: &ServerAddress,
    resolved: &Resolved,
    timeout: Duration,
) -> Result<LegacyStatus, StatusError> {
    let deadline = Instant::now() + timeout;
    let mut s = open(&resolved.addrs, deadline)?;
    arm(&s, deadline)?;
    s.write_all(&legacy_request(&typed.host, typed.port))?;
    let mut head = [0u8; 3];
    s.read_exact(&mut head)?;
    let n = u16::from_be_bytes([head[1], head[2]]) as usize;
    let mut rest = vec![0; 2 * n];
    s.read_exact(&mut rest)?;
    let mut all = head.to_vec();
    all.extend_from_slice(&rest);
    parse_legacy(&all).ok_or(StatusError::Invalid("legacy ping response"))
}

/// Full flow of a list entry: parsing, resolution, modern ping, and legacy ping
/// as a fallback if the modern connection fails before any response.
pub fn ping(input: &str, timeout: Duration) -> Result<PingOutcome, StatusError> {
    let typed = ServerAddress::parse(input);
    let resolved = resolve(&typed).ok_or(StatusError::UnknownHost)?;
    match query_resolved(&typed, &resolved, timeout) {
        Ok(p) => Ok(PingOutcome::Modern(p)),
        Err(e) => legacy_query(&typed, &resolved, timeout).map(PingOutcome::Legacy).map_err(|_| e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_forms() {
        let p = ServerAddress::try_parse;
        assert_eq!(p("example.com"), Some(ServerAddress::new("example.com", 25565)));
        assert_eq!(p("example.com:25570"), Some(ServerAddress::new("example.com", 25570)));
        assert_eq!(p("example.com:"), Some(ServerAddress::new("example.com", 25565)));
        assert_eq!(p("[::1]:25570"), Some(ServerAddress::new("::1", 25570)));
        assert_eq!(p("[::1]"), Some(ServerAddress::new("::1", 25565)));
        assert_eq!(p("::1"), Some(ServerAddress::new("::1", 25565)));
        assert_eq!(p("127.0.0.1:0"), Some(ServerAddress::new("127.0.0.1", 0)));
        assert_eq!(p(""), None);
        assert_eq!(p(":25565"), None);
        assert_eq!(p("host:65536"), None);
        assert_eq!(p("host:+5"), None);
        assert_eq!(p("host:abc"), None);
        assert_eq!(p("[::1]x"), None);
        assert_eq!(p("[::1"), None);
        assert_eq!(ServerAddress::parse("host:x"), ServerAddress::invalid());
        assert!(!ServerAddress::is_valid(""));
    }

    #[test]
    fn literal_ip_resolves_without_dns() {
        let r = resolve(&ServerAddress::new("127.0.0.1", 25565)).unwrap();
        assert_eq!(r.host, "127.0.0.1");
        assert_eq!(r.addrs, vec!["127.0.0.1:25565".parse().unwrap()]);
    }

    /// Blocked servers: reference SHA-1 fingerprint, exact name (case ignored, SRV prefix and
    /// trailing dot removed), subdomains of a `*.` pattern, IPv4 ranges `a.b.*`; refusal at
    /// resolution on the typed name or the obtained IP.
    #[test]
    fn blocked_servers_match_names_suffixes_and_ip_ranges() {
        assert_eq!(blocklist_hash("ABC"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let list: hashbrown::HashSet<String> =
            ["mc.bad.com", "*.evil.net", "127.0.*", "*.ünï.de"].iter().map(|n| blocklist_hash(n)).collect();
        for (host, want) in [
            ("mc.bad.com", true),
            ("MC.Bad.COM.", true),
            ("_minecraft._tcp.mc.bad.com", true),
            ("play.mc.bad.com", false),
            ("bad.com", false),
            // The pattern `*.evil.net` also covers the bare domain (the game tests `*.` + the name).
            ("evil.net", true),
            ("notevil.net", false),
            ("a.b.evil.net", true),
            ("127.0.0.1", true),
            ("127.1.0.1", false),
            ("x.127.0.0", false),
            ("a.ünï.de", true),
            ("", false),
        ] {
            assert_eq!(is_blocked(&list, host), want, "{host}");
        }
        // Nothing blocked: empty list (service unreachable).
        assert!(!is_blocked(&Default::default(), "mc.bad.com"));
        let blocked = |h: &str| is_blocked(&list, h);
        assert!(resolve_checked(&ServerAddress::new("127.0.0.1", 25570), &blocked).is_none());
        assert!(resolve_checked(&ServerAddress::new("mc.bad.com", 25570), &blocked).is_none());
        assert!(resolve_checked(&ServerAddress::new("10.0.0.1", 25570), &blocked).is_some());
        // Dual-stack host [IPv6, blocked IPv4]: the game keeps the IPv4, hence refusal.
        let v6: SocketAddr = "[2001:db8::1]:25565".parse().unwrap();
        let v4: SocketAddr = "127.0.0.5:25565".parse().unwrap();
        let addrs = game_pick(vec![v6, v4]);
        assert_eq!(addrs, vec![v4]);
        assert!(!resolved_allowed(&Resolved { host: "ok.example".into(), port: 25565, addrs }, &blocked));
        assert_eq!(game_pick(vec![v6]), vec![v6]);
        // Out of the basic plane (emoji): a single "?", like Java's encoder.
        assert_eq!(blocklist_hash("a\u{1F600}"), blocklist_hash("a?"));
        assert_eq!(blocklist_hash("a\u{263A}"), blocklist_hash("a?"));
        assert_eq!(java_ip("::1".parse().unwrap()), "0:0:0:0:0:0:0:1");
    }

    /// Minimal 64 × 64 RGBA PNG (encoded by the `png` crate).
    fn png64() -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 64, 64);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[200; 64 * 64 * 4]).unwrap();
        drop(w);
        out
    }

    #[test]
    fn vanilla_status() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png64());
        // Line break in the middle of the base64: removed before decoding.
        let fav = format!("data:image/png;base64,{}\\n{}", &b64[..10], &b64[10..]);
        let json = format!(
            r#"{{"description":{{"text":"A Minecraft Server"}},"players":{{"max":20,"online":1,"sample":[{{"id":"4566e69f-c907-48ee-8d71-d7ba5aa00d20","name":"Steve"}}]}},"version":{{"name":"26.3","protocol":777}},"favicon":"{fav}","enforcesSecureChat":true}}"#
        );
        let s = parse_status_json(&json);
        assert_eq!(plain_text(&s.description), "A Minecraft Server");
        let p = s.players.unwrap();
        assert_eq!((p.max, p.online), (20, 1));
        assert_eq!(p.sample[0].name, "Steve");
        assert_eq!(p.sample[0].id, 0x4566e69f_c907_48ee_8d71_d7ba5aa00d20);
        assert_eq!(s.version, Some(Version { name: "26.3".into(), protocol: 777 }));
        let f = s.favicon.unwrap();
        assert!(f.is_displayable());
        assert_eq!(f.rgba().unwrap().len(), 64 * 64 * 4);
        assert!(s.enforces_secure_chat);
    }

    #[test]
    fn paper_component_description() {
        let json = r#"{"version":{"name":"Paper 26.3","protocol":777},"players":{"max":100,"online":0},
            "description":{"extra":[{"color":"gold","bold":true,"text":"My "},{"text":"server"}],"text":""}}"#;
        let s = parse_status_json(json);
        assert!(s.description.is_object());
        assert_eq!(s.description["extra"][0]["color"], "gold");
        assert_eq!(plain_text(&s.description), "My server");
        assert_eq!(s.players.unwrap().sample, vec![]);
        assert!(s.favicon.is_none());
        assert!(!s.enforces_secure_chat);
    }

    #[test]
    fn tolerant_fields() {
        // Without favicon, empty sample, string description, incomplete version.
        let s = parse_status_json(
            r#"{"description":"§aHello","players":{"max":5,"online":3,"sample":[]},"version":{"name":"x"}}"#,
        );
        assert_eq!(plain_text(&s.description), "§aHello");
        assert_eq!(s.players.unwrap().sample.len(), 0);
        assert_eq!(s.version, None);
        // UUID without dashes: invalid `players` integer; favicon in another format: ignored.
        let s = parse_status_json(
            r#"{"players":{"max":5,"online":3,"sample":[{"id":"4566e69fc90748ee8d71d7ba5aa00d20","name":"a"}]},"favicon":"data:image/jpeg;base64,AAAA","description":5}"#,
        );
        assert_eq!(s.players, None);
        assert_eq!(s.favicon, None);
        assert_eq!(s.description, Value::Null);
        // Unreadable JSON.
        let s = parse_status_json("{");
        assert!(s.players.is_none() && s.version.is_none());
        // Anonymous player.
        let s = parse_status_json(
            r#"{"players":{"max":1,"online":1,"sample":[{"id":"00000000-0000-0000-0000-000000000000","name":"Anonymous Player"}]}}"#,
        );
        assert!(s.players.unwrap().sample[0].is_anonymous());
    }

    #[test]
    fn favicon_size_checks() {
        let mut big = png64();
        big[16..20].copy_from_slice(&2000u32.to_be_bytes());
        assert!(Favicon::from_png(big).is_none());
        let mut small = png64();
        small[16..20].copy_from_slice(&32u32.to_be_bytes());
        assert!(!Favicon::from_png(small).unwrap().is_displayable());
        assert!(Favicon::from_png(vec![1, 2, 3]).is_none());
    }

    #[test]
    fn legacy_roundtrip() {
        let req = legacy_request("localhost", 25565);
        assert_eq!(&req[..3], &[0xfe, 0x01, 0xfa]);
        assert_eq!(u16::from_be_bytes([req[27], req[28]]) as usize, req.len() - 29);
        assert_eq!(req[29], 127);
        let mut resp = vec![0xff];
        write_legacy_str(&mut resp, "§1\u{0}127\u{0}1.6.4\u{0}My MOTD\u{0}3\u{0}x");
        let l = parse_legacy(&resp).unwrap();
        assert_eq!(
            l,
            LegacyStatus { protocol: 127, version: "1.6.4".into(), motd: "My MOTD".into(), online: 3, max: -1 }
        );
        assert!(parse_legacy(&[0xfe, 0, 0]).is_none());
    }
}
