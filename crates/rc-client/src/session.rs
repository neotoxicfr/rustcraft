//! Connection to an online server (`docs/specs/authentication.md` § 2, § 5): encryption
//! request, "Minecraft-style" digest, `join` with the session services, RSA-encrypted
//! reply. Service addresses read from the discovery document, like the game.
//!
//! Rules: the token only goes to the services (verified HTTPS, Mojang/Microsoft hosts
//! only); no token, no shared secret, no request body in the logs.

use std::sync::OnceLock;
use std::time::Duration;

use rc_proto::Reader;
use rc_proto::varint::write_varint;
use rc_text::{Arg, Component};

use crate::auth::{AccessToken, authlog};

/// Encryption request (server `hello` of the login phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionRequest {
    /// Empty in vanilla since 1.7.
    pub server_id: String,
    /// Server RSA public key, DER `SubjectPublicKeyInfo`.
    pub public_key: Vec<u8>,
    /// Challenge (4 bytes in vanilla).
    pub challenge: Vec<u8>,
    /// The server verifies the session (`join` required).
    pub authenticate: bool,
}

pub fn read_encryption_request(r: &mut Reader<'_>) -> rc_proto::Result<EncryptionRequest> {
    use rc_proto::Decode;
    let server_id = rc_proto::read_str(r, 20)?.to_owned();
    let n = rc_proto::read_count(r, 1)?;
    let public_key = r.take(n)?.to_vec();
    let n = rc_proto::read_count(r, 1)?;
    let challenge = r.take(n)?.to_vec();
    let authenticate = bool::decode(r)?;
    Ok(EncryptionRequest { server_id, public_key, challenge, authenticate })
}

/// Encryption response (`key`): secret then challenge, encrypted.
pub fn write_key(w: &mut Vec<u8>, secret: &[u8], challenge: &[u8]) {
    for b in [secret, challenge] {
        write_varint(w, b.len() as i32);
        w.extend_from_slice(b);
    }
}

/// Digest sent to `join`: SHA-1(identifier ‖ secret ‖ key), read as a signed integer.
pub fn server_hash(server_id: &str, secret: &[u8], public_key: &[u8]) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    // Identifier in ISO-8859-1 (ASCII in practice; beyond, one byte per character ≤ U+00FF).
    let latin1: Vec<u8> = server_id.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).collect();
    ctx.update(&latin1);
    ctx.update(secret);
    ctx.update(public_key);
    signed_hex(ctx.finish().as_ref())
}

/// Big-endian bytes read as two's complement, written in lowercase hexadecimal without leading
/// zeros, `-` in front if negative.
pub fn signed_hex(bytes: &[u8]) -> String {
    let negative = bytes.first().is_some_and(|b| b & 0x80 != 0);
    let mut v = bytes.to_vec();
    if negative {
        // Absolute value: inversion then +1.
        let mut carry = true;
        for b in v.iter_mut().rev() {
            *b = !*b;
            if carry {
                let (s, c) = b.overflowing_add(1);
                *b = s;
                carry = c;
            }
        }
    }
    let hex: String = v.iter().map(|b| format!("{b:02x}")).collect();
    let digits = hex.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    if negative { format!("-{digits}") } else { digits.to_owned() }
}

/// Shared secret: 16 bytes from the system's cryptographic generator.
pub fn new_secret() -> Result<[u8; 16], String> {
    let mut s = [0u8; 16];
    getrandom::fill(&mut s).map_err(|e| format!("random generator: {e}"))?;
    Ok(s)
}

/// RSA PKCS#1 v1.5 encryption with the server's public key (DER `SubjectPublicKeyInfo`).
pub fn rsa_encrypt(public_key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    use rsa::pkcs8::DecodePublicKey;
    use rsa::traits::RandomizedEncryptor;
    let key = rsa::RsaPublicKey::from_public_key_der(public_key).map_err(|e| format!("server key: {e}"))?;
    let mut rng = rand_core::UnwrapErr(getrandom::SysRng);
    rsa::pkcs1v15::EncryptingKey::new(key)
        .encrypt_with_rng(&mut rng, data)
        .map_err(|e| format!("RSA encryption: {e}"))
}

const DISCOVERY: &str = "https://discovery.minecraftservices.com/minecraft/client";

/// `User-Agent` header of the game: that of Java's HTTP stack (the game sets none),
/// version of the Java environment supplied by the official launcher for 26.3 (taken on
/// 18/10/2026 from its `release` file).
pub const USER_AGENT: &str = "Java/25.0.1";
/// `Accept` header set by default by Java's HTTP stack (corrected form from Java 23).
pub const ACCEPT: &str = "text/html, image/gif, image/jpeg, */*; q=0.2";
/// JSON body of the requests to the services.
const JSON: &str = "application/json; charset=utf-8";

/// Proxy passed by the launcher (`--proxyHost`, `--proxyPort`, `--proxyUser`, `--proxyPass`).
/// Credentials masked in `Debug`, never logged.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct LauncherProxy {
    pub host: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub pass: Option<String>,
}

impl std::fmt::Debug for LauncherProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LauncherProxy")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user.as_ref().map(|_| "<masked>"))
            .field("pass", &self.pass.as_ref().map(|_| "<masked>"))
            .finish()
    }
}

impl LauncherProxy {
    /// SOCKS proxy like the game: only with a host; port 8080 by default; unreadable or
    /// out-of-range port → no proxy; credentials only if both are
    /// non-empty. The target is resolved locally (Java's stack does it before SOCKS).
    pub fn proxy(&self) -> Option<ureq::Proxy> {
        let host = self.host.as_deref()?;
        let port = match self.port.as_deref() {
            None => 8080,
            Some(p) => p.trim().parse::<u16>().ok()?,
        };
        let mut b = ureq::Proxy::builder(ureq::ProxyProtocol::Socks5).host(host).port(port).resolve_target(true);
        if let (Some(u), Some(p)) = (self.user.as_deref(), self.pass.as_deref())
            && !u.is_empty()
            && !p.is_empty()
        {
            b = b.username(u).password(p);
        }
        b.build().ok()
    }
}

/// Launcher settings for the services: proxy, game folder, `--disableMultiplayer`,
/// `--disableChat`.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub proxy: LauncherProxy,
    pub game_dir: Option<std::path::PathBuf>,
    pub disable_multiplayer: bool,
    pub disable_chat: bool,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Sets the settings once, at startup, before any request.
pub fn configure(c: Config) {
    let _ = CONFIG.set(c);
}

fn config() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// Chat key pair file of a profile in the game folder.
pub fn stored_profile_keys_path(game_dir: &std::path::Path, uuid: u128) -> std::path::PathBuf {
    let h = format!("{uuid:032x}");
    let dashed = format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]);
    game_dir.join("profilekeys").join(format!("{dashed}.json"))
}

/// Deletes `profilekeys/<uuid>.json` if it exists (the game never keeps it outside a
/// development environment).
pub fn forget_stored_profile_keys(uuid: u128) {
    if let Some(dir) = &config().game_dir {
        let _ = std::fs::remove_file(stored_profile_keys_path(dir, uuid));
    }
}

/// HTTP client like the game's: headers of Java's stack (no `Accept-Encoding`),
/// launcher proxy (never the environment's), verified TLS (default setting,
/// never disabled). `read`: read timeout (5 s for the services, like the game).
pub fn http_agent(read: Option<Duration>, status_as_error: bool) -> ureq::Agent {
    http_config()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(read)
        .timeout_recv_body(read)
        .http_status_as_error(status_as_error)
        .build()
        .into()
}

/// Common settings (Java headers, launcher proxy), to be completed by the caller.
pub fn http_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    static PROXY: OnceLock<Option<ureq::Proxy>> = OnceLock::new();
    let proxy = PROXY.get_or_init(|| config().proxy.proxy()).clone();
    ureq::Agent::config_builder()
        .proxy(proxy)
        .user_agent(USER_AGENT)
        .accept(ACCEPT)
        .accept_encoding(ureq::config::AutoHeaderValue::None)
}

/// HTTP client of the services: 5 s timeouts, error codes read as responses.
pub fn agent() -> ureq::Agent {
    http_agent(Some(Duration::from_secs(5)), false)
}

/// Only the Mojang/Microsoft services receive the token: HTTPS, known host.
pub fn trusted_service(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("https://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let host = authority.split(':').next().unwrap_or_default().to_ascii_lowercase();
    ["minecraftservices.com", "mojang.com"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Services discovery document (`docs/specs/authentication.md` § 1.7), like the
/// game: read at startup on a separate thread; failure → new attempt after 1, 2, 4… 64 min;
/// success → re-read every 24 h (a failure keeps the previous document). Without a document, the
/// services are unavailable: no fallback address.
struct Discovery {
    state: std::sync::Mutex<DiscoveryState>,
    ready: std::sync::Condvar,
}

struct DiscoveryState {
    started: bool,
    ready: bool,
    doc: Option<std::sync::Arc<serde_json::Value>>,
}

static DISCOVERY_STATE: Discovery = Discovery {
    state: std::sync::Mutex::new(DiscoveryState { started: false, ready: false, doc: None }),
    ready: std::sync::Condvar::new(),
};

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Delay before the next attempt: `failures` consecutive failures (document absent) or 24 h.
fn discovery_delay(have_doc: bool, failures: u32) -> Duration {
    if have_doc { Duration::from_secs(24 * 3600) } else { Duration::from_secs(60 << failures.min(6)) }
}

/// Starts the discovery (once; at startup, like the game).
pub fn start_discovery() {
    {
        let mut s = lock(&DISCOVERY_STATE.state);
        if s.started {
            return;
        }
        s.started = true;
    }
    let r = std::thread::Builder::new().name("services discovery".into()).spawn(|| {
        let mut failures = 0u32;
        loop {
            let fetched = fetch_discovery();
            let have_doc = {
                let mut s = lock(&DISCOVERY_STATE.state);
                match fetched {
                    Ok(doc) => {
                        authlog("services discovery: read");
                        s.doc = Some(std::sync::Arc::new(doc));
                    }
                    Err(e) => authlog(format!("services discovery: failure ({e})")),
                }
                s.ready = true;
                DISCOVERY_STATE.ready.notify_all();
                s.doc.is_some()
            };
            let wait = discovery_delay(have_doc, failures);
            if have_doc {
                failures = 0;
            } else {
                failures += 1;
            }
            std::thread::sleep(wait);
        }
    });
    if r.is_err() {
        let mut s = lock(&DISCOVERY_STATE.state);
        s.ready = true;
        DISCOVERY_STATE.ready.notify_all();
    }
}

fn fetch_discovery() -> Result<serde_json::Value, String> {
    let mut r = agent().get(DISCOVERY).call().map_err(|e| transport_kind(&e).to_owned())?;
    if r.status().as_u16() >= 400 {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    let body = r.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| e.to_string())?;
    let doc: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    // "Offline" document: same as an absence.
    if doc["environment"] == "offline" || !doc["discovery"].is_object() {
        return Err("offline document".into());
    }
    Ok(doc)
}

/// Current document; waits for the end of the first attempt. `None`: services unavailable.
pub fn discovery() -> Option<std::sync::Arc<serde_json::Value>> {
    start_discovery();
    let mut s = lock(&DISCOVERY_STATE.state);
    while !s.ready {
        s = DISCOVERY_STATE.ready.wait(s).unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    s.doc.clone()
}

/// `uri` address of a service in the document (`session`/`join`, `player`/`getCertificates`…).
/// Only Mojang/Microsoft hosts over HTTPS are admitted (the token may go there).
pub fn endpoint(service: &str, key: &str) -> Option<String> {
    endpoint_in(&*discovery()?, service, key)
}

fn endpoint_in(doc: &serde_json::Value, service: &str, key: &str) -> Option<String> {
    match doc["discovery"][service]["endpoints"][key]["uri"].as_str() {
        Some(u) if trusted_service(u) => Some(u.to_owned()),
        Some(_) => {
            authlog(format!("discovery: address {service}/{key} refused (unrecognised host)"));
            None
        }
        None => {
            authlog(format!("discovery: {service}/{key} absent"));
            None
        }
    }
}

/// Address prefixes admitted for profile textures (`profiles`/`getTexture`, `validUris`
/// without `{textureId}`); empty if the services are unavailable.
pub fn texture_prefixes() -> Vec<String> {
    discovery().map(|d| texture_prefixes_in(&d)).unwrap_or_default()
}

fn texture_prefixes_in(doc: &serde_json::Value) -> Vec<String> {
    doc["discovery"]["profiles"]["endpoints"]["getTexture"]["validUris"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| Some(u.as_str()?.replace("{textureId}", "")))
        .collect()
}

/// `join` failure, translated like the game ("Failed to log in: …").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    Unavailable,
    InvalidSession,
    InsufficientPrivileges,
    Banned,
    Other(String),
}

impl JoinError {
    pub fn component(&self) -> Component {
        let reason = match self {
            JoinError::Unavailable => Component::translate("disconnect.loginFailedInfo.serversUnavailable", vec![]),
            JoinError::InvalidSession => Component::translate("disconnect.loginFailedInfo.invalidSession", vec![]),
            JoinError::InsufficientPrivileges => {
                Component::translate("disconnect.loginFailedInfo.insufficientPrivileges", vec![])
            }
            JoinError::Banned => Component::translate("disconnect.loginFailedInfo.userBanned", vec![]),
            JoinError::Other(m) => Component::text(m.clone()),
        };
        Component::translate("disconnect.loginFailedInfo", vec![Arg::Component(reason)])
    }

    /// Classification of an error response from the services (`error`, `errorMessage`).
    fn from_response(status: u16, error: &str, message: &str) -> JoinError {
        let has = |s: &str| error.contains(s) || message.contains(s);
        if status >= 500 {
            JoinError::Unavailable
        } else if has("InsufficientPrivilegesException") {
            JoinError::InsufficientPrivileges
        } else if has("multiplayer.access.banned") || has("FORCED_USERNAME_CHANGE") || has("UserBannedException") {
            JoinError::Banned
        } else if status == 401 || has("ForbiddenOperationException") {
            JoinError::InvalidSession
        } else if !message.is_empty() {
            JoinError::Other(message.to_owned())
        } else if !error.is_empty() {
            JoinError::Other(error.to_owned())
        } else {
            JoinError::Other(format!("Status: {status}"))
        }
    }
}

/// `POST session/join`: the server will be able to verify that this profile does join `hash`.
pub fn join(token: &AccessToken, profile: u128, hash: &str) -> Result<(), JoinError> {
    // Services unavailable (no discovery document): refusal without a request, like the game.
    let Some(url) = endpoint("session", "join") else {
        authlog("join: services unavailable (discovery absent)");
        return Err(JoinError::Unavailable);
    };
    let url = &url;
    let body = serde_json::json!({
        "accessToken": token.expose(),
        "selectedProfile": format!("{profile:032x}"),
        "serverId": hash,
    })
    .to_string();
    authlog(format!("join: request to the session services ({})", host_of(url)));
    let resp = agent().post(url).header("Content-Type", JSON).send(&body);
    drop(body);
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            authlog(format!("join: services unreachable ({})", transport_kind(&e)));
            return Err(JoinError::Unavailable);
        }
    };
    let status = resp.status().as_u16();
    authlog(format!("join: HTTP {status}"));
    // The game's services client treats any code < 400 as a success.
    if status < 400 {
        return Ok(());
    }
    let text = resp.body_mut().with_config().limit(64 * 1024).read_to_string().unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let error = v["error"].as_str().unwrap_or_default();
    let message = v["errorMessage"].as_str().unwrap_or_default();
    // The error type alone (no secret in these fields, but not the whole body either).
    authlog(format!("join: refusal '{error}'"));
    Err(JoinError::from_response(status, error, message))
}

/// Host of an address (log).
pub fn host_of(url: &str) -> &str {
    url.strip_prefix("https://").unwrap_or(url).split(['/', '?']).next().unwrap_or_default()
}

/// Kind of a transport error, without full URL or detail.
pub fn transport_kind(e: &ureq::Error) -> &'static str {
    match e {
        ureq::Error::Timeout(_) => "timeout",
        ureq::Error::HostNotFound => "host not found",
        ureq::Error::ConnectionFailed => "connection failed",
        ureq::Error::Io(_) => "network error",
        _ => "HTTP/TLS error",
    }
}

// ---------------------------------------------------------------------------------------------
// Read-only account services (`docs/specs/authentication.md` § 1.10): account profile
// (skin in singleplayer, username to change), account properties (multiplayer, chat, bans),
// block list. Nothing is ever sent other than the token, to the intended services.
// ---------------------------------------------------------------------------------------------

/// Profile read from the session services: properties (signed `textures`) and actions requested
/// of the player.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileResult {
    pub id: u128,
    pub name: String,
    pub properties: Vec<rc_proto_v777::players::Property>,
    /// Username to change (username ban).
    pub forced_name_change: bool,
    /// Skin rejected by moderation.
    pub banned_skin: bool,
}

/// Reading of a profile from the session services (`id`, `name`, `properties`, `profileActions`).
pub fn parse_profile(v: &serde_json::Value) -> Option<ProfileResult> {
    let id = crate::auth::parse_uuid(v["id"].as_str()?)?;
    let name = v["name"].as_str()?.to_owned();
    let properties = v["properties"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(rc_proto_v777::players::Property {
                name: p["name"].as_str()?.to_owned(),
                value: p["value"].as_str()?.to_owned(),
                signature: p["signature"].as_str().map(str::to_owned),
            })
        })
        .collect();
    let action = |t: &str| v["profileActions"].as_array().is_some_and(|a| a.iter().any(|x| x["updateType"] == t));
    Some(ProfileResult {
        id,
        name,
        properties,
        forced_name_change: action("FORCED_NAME_CHANGE"),
        banned_skin: action("USING_BANNED_SKIN"),
    })
}

/// Address of profile `id`: `?unsigned=false` if the properties must be signed.
fn profile_url(template: &str, id: u128, secure: bool) -> String {
    let url = template.replace("{profileId}", &format!("{id:032x}"));
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}unsigned={}", !secure)
}

/// Profile `id` (without token). `None`: unknown profile, services unavailable or error.
pub fn fetch_profile(id: u128, secure: bool) -> Option<ProfileResult> {
    let url = profile_url(&endpoint("session", "getProfileById")?, id, secure);
    let mut r = agent().get(&url).call().ok()?;
    if r.status().as_u16() >= 400 {
        return None;
    }
    let body = r.body_mut().with_config().limit(1 << 20).read_to_string().ok()?;
    if body.is_empty() {
        return None;
    }
    parse_profile(&serde_json::from_str(&body).ok()?)
}

/// Ban of a scope (`MULTIPLAYER`…) in the account properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BanDetails {
    pub id: String,
    /// End date (ISO 8601); absent: permanent.
    pub expires: Option<String>,
    /// Reason (numeric code as a string, see `gui.banned.reason.*`).
    pub reason: Option<String>,
    pub reason_message: Option<String>,
}

/// Flags of the account properties.
pub mod flag {
    pub const CHAT_ALLOWED: u32 = 1;
    pub const SERVERS_ALLOWED: u32 = 1 << 1;
    pub const REALMS_ALLOWED: u32 = 1 << 2;
    pub const TELEMETRY_ENABLED: u32 = 1 << 3;
    pub const OPTIONAL_TELEMETRY_AVAILABLE: u32 = 1 << 4;
    pub const PROFANITY_FILTER_ENABLED: u32 = 1 << 5;
    pub const FRIENDS_ENABLED: u32 = 1 << 6;
    pub const ACCEPT_FRIEND_INVITES: u32 = 1 << 7;
    pub const CHAT_FRIENDS_ONLY: u32 = 1 << 8;
}

/// Account properties (`player/attributes`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserProperties {
    pub flags: u32,
    /// Scope → ban.
    pub banned: Vec<(String, BanDetails)>,
}

impl UserProperties {
    /// Default properties when the services do not respond: chat, Realms and servers
    /// allowed, no ban (like the game).
    pub fn offline() -> UserProperties {
        UserProperties { flags: flag::CHAT_ALLOWED | flag::REALMS_ALLOWED | flag::SERVERS_ALLOWED, banned: Vec::new() }
    }

    pub fn flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    pub fn ban(&self, scope: &str) -> Option<&BanDetails> {
        self.banned.iter().find(|(s, _)| s == scope).map(|(_, b)| b)
    }
}

/// Reading of the `player/attributes` response, like the game.
pub fn parse_attributes(v: &serde_json::Value) -> UserProperties {
    let mut flags = 0;
    let p = &v["privileges"];
    for (k, f) in [
        ("onlineChat", flag::CHAT_ALLOWED),
        ("multiplayerServer", flag::SERVERS_ALLOWED),
        ("multiplayerRealms", flag::REALMS_ALLOWED),
        ("telemetry", flag::TELEMETRY_ENABLED),
        ("optionalTelemetry", flag::OPTIONAL_TELEMETRY_AVAILABLE),
    ] {
        if p[k]["enabled"].as_bool() == Some(true) {
            flags |= f;
        }
    }
    if v["profanityFilterPreferences"]["profanityFilterOn"].as_bool() == Some(true) {
        flags |= flag::PROFANITY_FILTER_ENABLED;
    }
    let friends = &v["friendsPreferences"];
    if friends["friends"] == "ENABLED" {
        flags |= flag::FRIENDS_ENABLED;
    }
    if friends["acceptInvites"] == "ENABLED" {
        flags |= flag::ACCEPT_FRIEND_INVITES;
    }
    match v["chatPreferences"]["textCommunication"].as_str() {
        Some("ENABLED") => flags |= flag::CHAT_ALLOWED,
        Some("FRIENDS_ONLY") => flags |= flag::CHAT_ALLOWED | flag::CHAT_FRIENDS_ONLY,
        _ => {}
    }
    let text = |x: &serde_json::Value| x.as_str().map(str::to_owned);
    let banned = v["banStatus"]["bannedScopes"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(scope, b)| {
            let details = BanDetails {
                id: b["banId"].as_str().unwrap_or_default().to_owned(),
                expires: text(&b["expires"]),
                reason: text(&b["reason"]),
                reason_message: text(&b["reasonMessage"]),
            };
            (scope.clone(), details)
        })
        .collect();
    UserProperties { flags, banned }
}

/// `GET player/attributes` (Bearer); any failure → offline properties.
fn fetch_properties(token: &AccessToken) -> UserProperties {
    let fetched = (|| -> Result<UserProperties, String> {
        let url = endpoint("player", "getAttributes").ok_or("services unavailable")?;
        let mut r = agent()
            .get(&url)
            .header("Authorization", &format!("Bearer {}", token.expose()))
            .call()
            .map_err(|e| transport_kind(&e).to_owned())?;
        let status = r.status().as_u16();
        if status >= 400 {
            return Err(format!("HTTP {status}"));
        }
        let body = r.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|_| "unreadable body")?;
        if body.is_empty() {
            return Ok(UserProperties::default());
        }
        Ok(parse_attributes(&serde_json::from_str(&body).map_err(|_| "unreadable JSON")?))
    })();
    match fetched {
        Ok(p) => {
            authlog(format!(
                "account properties: read (flags {:#x}, {} ban(s))",
                p.flags,
                p.banned.len()
            ));
            p
        }
        Err(e) => {
            authlog(format!("account properties: failure ({e}), offline values"));
            UserProperties::offline()
        }
    }
}

/// Result awaited from a thread (like an asynchronous task of the game).
struct Pending<T> {
    value: std::sync::Mutex<Option<T>>,
    done: std::sync::Condvar,
}

impl<T: Clone> Pending<T> {
    const fn new() -> Self {
        Pending { value: std::sync::Mutex::new(None), done: std::sync::Condvar::new() }
    }

    fn set(&self, v: T) {
        *lock(&self.value) = Some(v);
        self.done.notify_all();
    }

    fn get_now(&self) -> Option<T> {
        lock(&self.value).clone()
    }

    fn join(&self) -> T {
        let mut g = lock(&self.value);
        loop {
            if let Some(v) = g.as_ref() {
                return v.clone();
            }
            g = self.done.wait(g).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

static OWN_PROFILE: Pending<Option<ProfileResult>> = Pending::new();
static PROPERTIES: Pending<UserProperties> = Pending::new();
static ACCOUNT_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Startup, like the game: discovery, then account profile and account properties in
/// parallel on separate threads. Without a launcher account: no profile, offline properties,
/// no request.
pub fn start_account_services() {
    if ACCOUNT_STARTED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let Some(account) = crate::auth::account() else {
        OWN_PROFILE.set(None);
        PROPERTIES.set(UserProperties::offline());
        return;
    };
    start_discovery();
    let id = account.uuid;
    let spawned = std::thread::Builder::new().name("account profile".into()).spawn(move || {
        let p = fetch_profile(id, true);
        authlog(format!("account profile: {}", if p.is_some() { "read" } else { "absent" }));
        OWN_PROFILE.set(p);
    });
    if spawned.is_err() {
        OWN_PROFILE.set(None);
    }
    let token = account.token.clone();
    let spawned = std::thread::Builder::new().name("account properties".into()).spawn(move || {
        // Without a usable token, the game would be refused (401) and take the offline values.
        PROPERTIES.set(token.as_ref().map_or_else(UserProperties::offline, fetch_properties));
    });
    if spawned.is_err() {
        PROPERTIES.set(UserProperties::offline());
    }
}

/// Account profile (waits for its reading). `None`: no account or profile not found.
pub fn own_profile() -> Option<ProfileResult> {
    start_account_services();
    OWN_PROFILE.join()
}

/// Account properties (waits for their reading).
pub fn user_properties() -> UserProperties {
    start_account_services();
    PROPERTIES.join()
}

/// Username to change, from the profile if already read (like the game, without waiting).
pub fn is_name_banned() -> bool {
    OWN_PROFILE.get_now().flatten().is_some_and(|p| p.forced_name_change)
}

/// Current multiplayer ban.
pub fn multiplayer_ban() -> Option<BanDetails> {
    user_properties().ban("MULTIPLAYER").cloned()
}

/// Multiplayer allowed: not `--disableMultiplayer`, servers allowed by the account, neither
/// a multiplayer ban nor a username to change.
pub fn allows_multiplayer() -> bool {
    !config().disable_multiplayer
        && user_properties().flag(flag::SERVERS_ALLOWED)
        && multiplayer_ban().is_none()
        && !is_name_banned()
}

/// Reason displayed on the multiplayer buttons of the title screen (translation key), or
/// `None` if multiplayer is allowed.
pub fn multiplayer_disabled_reason() -> Option<&'static str> {
    if allows_multiplayer() {
        None
    } else if is_name_banned() {
        Some("title.multiplayer.disabled.banned.name")
    } else if let Some(b) = multiplayer_ban() {
        Some(if b.expires.is_some() {
            "title.multiplayer.disabled.banned.temporary"
        } else {
            "title.multiplayer.disabled.banned.permanent"
        })
    } else {
        Some("title.multiplayer.disabled")
    }
}

/// Chat disabled by the launcher (`--disableChat`): restriction on a multiplayer server.
pub fn chat_disabled_by_launcher() -> bool {
    config().disable_chat
}

/// Chat not allowed by the account: restriction on a multiplayer server.
pub fn chat_disabled_by_profile() -> bool {
    !user_properties().flag(flag::CHAT_ALLOWED)
}

/// Player messages allowed on a multiplayer server: neither `--disableChat`, nor chat refused
/// by the account ("disabled by the launcher / by the profile" restrictions of the game).
pub fn player_chat_allowed() -> bool {
    !chat_disabled_by_launcher() && !chat_disabled_by_profile()
}

/// Account block list (`privacy/blocklist`), like the game: re-read at each connection
/// to a server (at most every 120 s), consulted online only.
struct BlockList {
    list: Option<hashbrown::HashSet<u128>>,
    next_request: Option<std::time::Instant>,
    /// Online connections open.
    online: u32,
    refreshing: bool,
}

static BLOCKS: std::sync::Mutex<BlockList> =
    std::sync::Mutex::new(BlockList { list: None, next_request: None, online: 0, refreshing: false });
static BLOCKS_DONE: std::sync::Condvar = std::sync::Condvar::new();

const BLOCKLIST_COOLDOWN: Duration = Duration::from_secs(120);

/// Reading of the `privacy/blocklist` response (`blockedProfiles`, UUIDs with dashes).
pub fn parse_blocklist(v: &serde_json::Value) -> hashbrown::HashSet<u128> {
    v["blockedProfiles"].as_array().into_iter().flatten().filter_map(|u| crate::auth::parse_uuid(u.as_str()?)).collect()
}

/// List request (Bearer). `None`: failure (the previous list is forgotten, like the game).
fn fetch_blocklist() -> Option<hashbrown::HashSet<u128>> {
    let token = crate::auth::account()?.token.clone()?;
    let url = endpoint("player", "getBlocklist")?;
    let mut r = agent().get(&url).header("Authorization", &format!("Bearer {}", token.expose())).call().ok()?;
    if r.status().as_u16() >= 400 {
        authlog(format!("block list: HTTP {}", r.status().as_u16()));
        return None;
    }
    let body = r.body_mut().with_config().limit(1 << 20).read_to_string().ok()?;
    if body.is_empty() {
        return Some(Default::default());
    }
    let list = parse_blocklist(&serde_json::from_str(&body).ok()?);
    authlog(format!("block list: {} profile(s)", list.len()));
    Some(list)
}

fn can_request(b: &BlockList) -> bool {
    b.next_request.is_none_or(|t| std::time::Instant::now() >= t)
}

/// Forced request, lock released during the call.
fn force_fetch(mut b: std::sync::MutexGuard<'_, BlockList>) -> std::sync::MutexGuard<'_, BlockList> {
    b.next_request = Some(std::time::Instant::now() + BLOCKLIST_COOLDOWN);
    drop(b);
    let list = fetch_blocklist();
    let mut b = lock(&BLOCKS);
    b.list = list;
    b
}

/// Online entry (connection to a server): list re-read in a background task.
pub fn start_online_mode() {
    let mut b = lock(&BLOCKS);
    b.online += 1;
    if crate::auth::account().and_then(|a| a.token.as_ref()).is_none() || b.refreshing {
        return;
    }
    b.refreshing = true;
    drop(b);
    let r = std::thread::Builder::new().name("block list".into()).spawn(|| {
        let b = lock(&BLOCKS);
        let mut b = if b.list.is_none() || can_request(&b) { force_fetch(b) } else { b };
        b.refreshing = false;
        BLOCKS_DONE.notify_all();
    });
    if r.is_err() {
        lock(&BLOCKS).refreshing = false;
    }
}

/// End of an online connection.
pub fn stop_online_mode() {
    let mut b = lock(&BLOCKS);
    b.online = b.online.saturating_sub(1);
}

/// Player blocked by the account (online only; never the nil UUID).
pub fn is_blocked(uuid: u128) -> bool {
    let mut b = lock(&BLOCKS);
    if b.online == 0 {
        return false;
    }
    while b.refreshing {
        b = BLOCKS_DONE.wait(b).unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    if uuid == 0 {
        return false;
    }
    if b.list.is_none() {
        if !can_request(&b) || crate::auth::account().and_then(|a| a.token.as_ref()).is_none() {
            return false;
        }
        b = force_fetch(b);
    }
    b.list.as_ref().is_some_and(|l| l.contains(&uuid))
}

/// Reply to the encryption request: `join` if the server requires it (and we have a token),
/// then secret and challenge encrypted. Returns the `key` packet to send and the secret to enable.
pub fn answer(
    req: &EncryptionRequest,
    token: Option<&AccessToken>,
    profile: u128,
) -> Result<(Vec<u8>, [u8; 16]), Box<Component>> {
    let protocol_error = |e: String| {
        authlog(format!("encryption: {e}"));
        Box::new(Component::translate("disconnect.loginFailedInfo", vec![Arg::Component(Component::text(e))]))
    };
    authlog(format!(
        "encryption request: key {} bytes, challenge {} bytes, authentication {}",
        req.public_key.len(),
        req.challenge.len(),
        if req.authenticate { "required" } else { "not required" }
    ));
    let secret = new_secret().map_err(protocol_error)?;
    if req.authenticate {
        let hash = server_hash(&req.server_id, &secret, &req.public_key);
        match token {
            // No token (offline username, or launcher filler value): the game
            // would try `join` and be refused; we send nothing and give the same reason.
            None => {
                authlog("join: no access token (launch from a connected launcher)");
                return Err(Box::new(JoinError::InvalidSession.component()));
            }
            Some(t) => join(t, profile, &hash).map_err(|e| Box::new(e.component()))?,
        }
        authlog("join: accepted");
    }
    let secret_enc = rsa_encrypt(&req.public_key, &secret).map_err(protocol_error)?;
    let challenge_enc = rsa_encrypt(&req.public_key, &req.challenge).map_err(protocol_error)?;
    let mut body = Vec::with_capacity(secret_enc.len() + challenge_enc.len() + 6);
    write_key(&mut body, &secret_enc, &challenge_enc);
    authlog("encryption reply ready, AES-128 CFB8 enabled after sending");
    Ok((body, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha1_hex(s: &str) -> String {
        signed_hex(ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, s.as_bytes()).as_ref())
    }

    #[test]
    fn minecraft_digest_public_vectors() {
        assert_eq!(sha1_hex("Notch"), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(sha1_hex("jeb_"), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(sha1_hex("simon"), "88e16a1019277b15d58faf0541e11910eb756f6");
        // Concatenation identifier ‖ secret ‖ key.
        assert_eq!(server_hash("No", b"tc", b"h"), sha1_hex("Notch"));
    }

    #[test]
    fn signed_hex_edges() {
        assert_eq!(signed_hex(&[0, 0, 1]), "1");
        assert_eq!(signed_hex(&[0, 0, 0]), "0");
        assert_eq!(signed_hex(&[0xff, 0xff]), "-1");
        assert_eq!(signed_hex(&[0x80, 0x00]), "-8000");
    }

    #[test]
    fn encryption_packets_round_trip() {
        let mut w = Vec::new();
        rc_proto::write_str(&mut w, "");
        write_varint(&mut w, 3);
        w.extend_from_slice(&[1, 2, 3]);
        write_varint(&mut w, 4);
        w.extend_from_slice(&[9, 8, 7, 6]);
        w.push(1);
        let req = read_encryption_request(&mut Reader::new(&w)).expect("reading");
        assert_eq!(
            req,
            EncryptionRequest {
                server_id: String::new(),
                public_key: vec![1, 2, 3],
                challenge: vec![9, 8, 7, 6],
                authenticate: true
            }
        );
        let mut k = Vec::new();
        write_key(&mut k, &[0xaa; 128], &[0xbb; 128]);
        assert_eq!(k.len(), 2 + 128 + 2 + 128);
        assert_eq!(&k[..2], &[0x80, 0x01]);
    }

    #[test]
    fn rsa_pkcs1_and_cfb8_round_trip() {
        use rsa::pkcs8::EncodePublicKey;
        use rsa::traits::Decryptor;
        let mut rng = rand_core::UnwrapErr(getrandom::SysRng);
        let private = rsa::RsaPrivateKey::new(&mut rng, 1024).expect("test key");
        let der = private.to_public_key().to_public_key_der().expect("DER").as_bytes().to_vec();
        let secret = new_secret().expect("secret");
        let enc = rsa_encrypt(&der, &secret).expect("encryption");
        assert_eq!(enc.len(), 128);
        let dec = rsa::pkcs1v15::DecryptingKey::new(private).decrypt(&enc).expect("decryption");
        assert_eq!(dec, secret);
        // AES-128 CFB8: key = IV = secret, continuous state in both directions.
        let (mut a, mut b) = (rc_net::Cfb8::new(&secret), rc_net::Cfb8::new(&secret));
        let plain = b"test packet long enough for several AES blocks".to_vec();
        let mut data = plain.clone();
        a.encrypt(&mut data[..10]);
        a.encrypt(&mut data[10..]);
        assert_ne!(data, plain);
        b.decrypt(&mut data);
        assert_eq!(data, plain);
    }

    #[test]
    fn only_mojang_services_get_the_token() {
        assert!(trusted_service("https://sessionserver.mojang.com/session/minecraft/join"));
        assert!(trusted_service("https://api.minecraftservices.com/player/certificates"));
        assert!(!trusted_service("http://sessionserver.mojang.com/session/minecraft/join"));
        assert!(!trusted_service("https://evil.example/mojang.com"));
        assert!(!trusted_service("https://user@evil.example"));
        assert!(!trusted_service("https://notmojang.com/x"));
    }

    #[test]
    fn join_errors() {
        assert_eq!(JoinError::from_response(503, "", ""), JoinError::Unavailable);
        assert_eq!(
            JoinError::from_response(403, "ForbiddenOperationException", "Invalid token"),
            JoinError::InvalidSession
        );
        assert_eq!(JoinError::from_response(401, "", ""), JoinError::InvalidSession);
        assert_eq!(
            JoinError::from_response(403, "InsufficientPrivilegesException", ""),
            JoinError::InsufficientPrivileges
        );
        assert_eq!(JoinError::from_response(403, "", "multiplayer.access.banned"), JoinError::Banned);
        assert_eq!(JoinError::from_response(400, "X", "Bad"), JoinError::Other("Bad".into()));
        assert_eq!(JoinError::from_response(418, "", ""), JoinError::Other("Status: 418".into()));
    }

    /// Discovery document taken on 18/10/2026 (excerpt).
    fn discovery_doc() -> serde_json::Value {
        serde_json::json!({
            "environment": "prod",
            "discovery": {
                "session": { "endpoints": {
                    "join": { "uri": "https://sessionserver.mojang.com/session/minecraft/join" },
                    "getProfileById": { "uri": "https://sessionserver.mojang.com/session/minecraft/profile/{profileId}" }
                }},
                "player": { "endpoints": {
                    "getAttributes": { "uri": "https://api.minecraftservices.com/player/attributes" },
                    "getBlocklist": { "uri": "https://evil.example/privacy/blocklist" }
                }},
                "profiles": { "endpoints": { "getTexture": { "validUris": [
                    "https://textures.minecraft.net/texture/{textureId}",
                    "http://textures.minecraft.net/texture/{textureId}"
                ]}}}
            }
        })
    }

    #[test]
    fn endpoints_come_from_discovery_without_fallback() {
        let d = discovery_doc();
        assert_eq!(
            endpoint_in(&d, "session", "join").as_deref(),
            Some("https://sessionserver.mojang.com/session/minecraft/join")
        );
        // Absent: no invented address (the game then treats the services as unavailable).
        assert_eq!(endpoint_in(&d, "player", "getCertificates"), None);
        assert_eq!(endpoint_in(&serde_json::Value::Null, "session", "join"), None);
        // Non-Mojang host: refused (the token will never go there).
        assert_eq!(endpoint_in(&d, "player", "getBlocklist"), None);
        assert_eq!(
            texture_prefixes_in(&d),
            vec!["https://textures.minecraft.net/texture/", "http://textures.minecraft.net/texture/"]
        );
    }

    #[test]
    fn discovery_retry_schedule() {
        let m = |s: u64| Duration::from_secs(60 * s);
        assert_eq!(discovery_delay(false, 0), m(1));
        assert_eq!(discovery_delay(false, 1), m(2));
        assert_eq!(discovery_delay(false, 6), m(64));
        assert_eq!(discovery_delay(false, 30), m(64));
        assert_eq!(discovery_delay(true, 3), Duration::from_secs(86_400));
    }

    #[test]
    fn profile_url_and_parse() {
        let t = "https://sessionserver.mojang.com/session/minecraft/profile/{profileId}";
        assert_eq!(
            profile_url(t, 0x069a79f444e94726a5befca90e38aaf5, true),
            "https://sessionserver.mojang.com/session/minecraft/profile/069a79f444e94726a5befca90e38aaf5?unsigned=false"
        );
        assert!(profile_url(t, 1, false).ends_with("?unsigned=true"));
        let v = serde_json::json!({
            "id": "069a79f444e94726a5befca90e38aaf5",
            "name": "Notch",
            "properties": [{ "name": "textures", "value": "e30=", "signature": "c2ln" }],
            "profileActions": [{ "updateType": "FORCED_NAME_CHANGE" }]
        });
        let p = parse_profile(&v).expect("profile");
        assert_eq!(p.id, 0x069a79f444e94726a5befca90e38aaf5);
        assert_eq!(p.properties.len(), 1);
        assert_eq!(p.properties[0].signature.as_deref(), Some("c2ln"));
        assert!(p.forced_name_change && !p.banned_skin);
    }

    #[test]
    fn attributes_like_the_game() {
        let v = serde_json::json!({
            "privileges": {
                "onlineChat": { "enabled": false },
                "multiplayerServer": { "enabled": true },
                "multiplayerRealms": { "enabled": true },
                "telemetry": { "enabled": true },
                "optionalTelemetry": { "enabled": false }
            },
            "profanityFilterPreferences": { "profanityFilterOn": true },
            "friendsPreferences": { "friends": "ENABLED", "acceptInvites": "DISABLED" },
            "chatPreferences": { "textCommunication": "FRIENDS_ONLY" },
            "banStatus": { "bannedScopes": { "MULTIPLAYER": {
                "banId": "1b2c", "expires": "2026-11-01T00:00:00Z", "reason": "3", "reasonMessage": null
            }}}
        });
        let p = parse_attributes(&v);
        // Chat allowed by the "friends only" preferences despite the revoked privilege.
        assert!(p.flag(flag::CHAT_ALLOWED) && p.flag(flag::CHAT_FRIENDS_ONLY));
        assert!(p.flag(flag::SERVERS_ALLOWED) && p.flag(flag::REALMS_ALLOWED) && p.flag(flag::TELEMETRY_ENABLED));
        assert!(!p.flag(flag::OPTIONAL_TELEMETRY_AVAILABLE) && !p.flag(flag::ACCEPT_FRIEND_INVITES));
        assert!(p.flag(flag::PROFANITY_FILTER_ENABLED) && p.flag(flag::FRIENDS_ENABLED));
        let ban = p.ban("MULTIPLAYER").expect("ban");
        assert_eq!(ban.expires.as_deref(), Some("2026-11-01T00:00:00Z"));
        assert_eq!(ban.reason.as_deref(), Some("3"));
        assert_eq!(ban.reason_message, None);
        // Empty response: no flags; failure: offline values.
        assert_eq!(parse_attributes(&serde_json::json!({})).flags, 0);
        let off = UserProperties::offline();
        assert!(off.flag(flag::CHAT_ALLOWED) && off.flag(flag::SERVERS_ALLOWED) && off.flag(flag::REALMS_ALLOWED));
        assert!(!off.flag(flag::TELEMETRY_ENABLED) && off.banned.is_empty());
    }

    #[test]
    fn blocklist_parse() {
        let v = serde_json::json!({ "blockedProfiles": ["069a79f4-44e9-4726-a5be-fca90e38aaf5", "bad"] });
        let l = parse_blocklist(&v);
        assert_eq!(l.len(), 1);
        assert!(l.contains(&0x069a79f444e94726a5befca90e38aaf5));
        // Offline (no connection to a server): nobody is blocked, no request.
        assert!(!is_blocked(0x069a79f444e94726a5befca90e38aaf5));
    }

    #[test]
    fn launcher_proxy_like_the_game() {
        let p = |host: Option<&str>, port: Option<&str>, user: Option<&str>, pass: Option<&str>| LauncherProxy {
            host: host.map(Into::into),
            port: port.map(Into::into),
            user: user.map(Into::into),
            pass: pass.map(Into::into),
        };
        assert!(p(None, Some("1080"), None, None).proxy().is_none());
        let d = p(Some("127.0.0.1"), None, None, None).proxy().expect("proxy");
        assert_eq!((d.host(), d.port(), d.protocol()), ("127.0.0.1", 8080, ureq::ProxyProtocol::Socks5));
        assert!(d.resolve_target() && d.username().is_none());
        assert!(p(Some("h"), Some("http"), None, None).proxy().is_none());
        assert!(p(Some("h"), Some("70000"), None, None).proxy().is_none());
        // Credentials only if both are non-empty.
        assert!(p(Some("h"), Some("1080"), Some("u"), Some("")).proxy().expect("m").username().is_none());
        let a = p(Some("h"), Some("1080"), Some("u"), Some("s3cret")).proxy().expect("m");
        assert_eq!((a.username(), a.password()), (Some("u"), Some("s3cret")));
        assert!(!format!("{:?}", p(Some("h"), None, Some("u"), Some("s3cret"))).contains("s3cret"));
    }

    #[test]
    fn requests_carry_the_java_headers() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let port = listener.local_addr().expect("address").port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("connection");
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).expect("read");
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").expect("reply");
            String::from_utf8_lossy(&req).to_ascii_lowercase()
        });
        agent().get(&format!("http://127.0.0.1:{port}/x")).call().expect("request");
        let req = server.join().expect("thread");
        assert!(req.contains("user-agent: java/25.0.1\r\n"), "{req}");
        assert!(req.contains("accept: text/html, image/gif, image/jpeg, */*; q=0.2\r\n"), "{req}");
        assert!(!req.contains("accept-encoding"), "{req}");
        assert!(!req.contains("rustcraft") && !req.contains("ureq"), "{req}");
    }

    #[test]
    fn stored_keys_path_is_the_game_one() {
        let path = stored_profile_keys_path(std::path::Path::new("game"), 0x069a79f444e94726a5befca90e38aaf5);
        assert_eq!(path, std::path::Path::new("game/profilekeys/069a79f4-44e9-4726-a5be-fca90e38aaf5.json"));
    }

    #[test]
    fn no_token_means_invalid_session_without_network() {
        let req =
            EncryptionRequest { server_id: String::new(), public_key: vec![], challenge: vec![], authenticate: true };
        assert_eq!(*answer(&req, None, 1).unwrap_err(), JoinError::InvalidSession.component());
    }
}
