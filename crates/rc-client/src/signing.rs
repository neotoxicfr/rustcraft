//! Secure chat (`docs/specs/authentication.md` § 3, `chat.md` § 1.7, § 2): player key pair
//! (`player/certificates`), chat session, signing of messages and command
//! arguments, cache of received signatures and tracking of the "last seen".
//!
//! Only on an online server. The private key stays in memory (like the official game),
//! is never logged; the token is only used for the request to the services.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use rc_proto::varint::write_varint;

use crate::auth::{AccessToken, authlog};

/// 256-byte RSA signature (2048-bit key).
pub type Sig = [u8; 256];

/// Chat key pair issued by the services.
pub struct ProfileKeys {
    private: ring::signature::RsaKeyPair,
    /// Public key, DER `SubjectPublicKeyInfo`.
    pub public_der: Vec<u8>,
    /// v2 signature of the key by the services.
    pub signature: Vec<u8>,
    pub expires_at_ms: i64,
    pub refreshed_after_ms: i64,
}

impl std::fmt::Debug for ProfileKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileKeys")
            .field("public_der", &self.public_der.len())
            .field("expires_at_ms", &self.expires_at_ms)
            .field("refreshed_after_ms", &self.refreshed_after_ms)
            .finish_non_exhaustive()
    }
}

impl ProfileKeys {
    /// Reading of the `player/certificates` response (PKCS#8 private key and X.509 public key
    /// under "RSA" PEM headers, v2 signature in base 64, ISO 8601 dates).
    pub fn from_json(v: &serde_json::Value) -> Result<ProfileKeys, String> {
        let pem = |k: &str| -> Result<Vec<u8>, String> {
            let s = v["keyPair"][k].as_str().ok_or_else(|| format!("{k} missing"))?;
            pem_body(s).ok_or_else(|| format!("{k} unreadable"))
        };
        let private_der = pem("privateKey")?;
        let public_der = pem("publicKey")?;
        let private = ring::signature::RsaKeyPair::from_pkcs8(&private_der).map_err(|e| format!("private key: {e}"))?;
        if private.public().modulus_len() != 256 {
            return Err("key of unexpected size".into());
        }
        let signature = v["publicKeySignatureV2"]
            .as_str()
            .filter(|s| !s.is_empty())
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .filter(|s| !s.is_empty())
            .ok_or("v2 signature missing")?;
        let date = |k: &str| v[k].as_str().and_then(parse_iso8601_ms).ok_or_else(|| format!("{k} unreadable"));
        if public_der.is_empty() || public_der.len() > 512 || signature.len() > 4096 {
            return Err("public key or signature out of bounds".into());
        }
        Ok(ProfileKeys {
            private,
            public_der,
            signature,
            expires_at_ms: date("expiresAt")?,
            refreshed_after_ms: date("refreshedAfter")?,
        })
    }

    /// SHA256withRSA (PKCS#1 v1.5) of the content.
    pub fn sign(&self, data: &[u8]) -> Option<Sig> {
        let mut sig = [0u8; 256];
        let rng = ring::rand::SystemRandom::new();
        self.private.sign(&ring::signature::RSA_PKCS1_SHA256, &rng, data, &mut sig).ok()?;
        Some(sig)
    }

    fn due_refresh(&self, now_ms: i64) -> bool {
        now_ms >= self.refreshed_after_ms
    }
}

/// Base 64 content of a PEM block (split lines allowed).
fn pem_body(s: &str) -> Option<Vec<u8>> {
    let body: String =
        s.lines().filter(|l| !l.starts_with("-----")).flat_map(|l| l.chars()).filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(body).ok()
}

/// ISO 8601 date (`2026-10-13T08:15:30.123456Z`, or `±hh:mm` offset) in Unix ms.
pub fn parse_iso8601_ms(s: &str) -> Option<i64> {
    let (date, time) = s.split_once(['T', 't'])?;
    let mut d = date.split('-');
    let (y, mo, da): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let (clock, offset_s) = if let Some(c) = time.strip_suffix(['Z', 'z']) {
        (c, 0)
    } else {
        let i = time.rfind(['+', '-'])?;
        let (c, off) = time.split_at(i);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let (oh, om) = off[1..].split_once(':').unwrap_or_else(|| (&off[1..3.min(off.len())], "0"));
        (c, sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60))
    };
    let (hms, frac) = clock.split_once('.').unwrap_or((clock, ""));
    let mut t = hms.split(':');
    let (h, mi, se): (i64, i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let ms: i64 = if frac.is_empty() { 0 } else { format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()? };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    // Days since the epoch (proleptic Gregorian calendar).
    let (yy, m) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * m + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + h * 3600 + mi * 60 + se) - offset_s) * 1000 + ms)
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// `i64` from the system's cryptographic generator (message salt).
pub fn random_i64() -> i64 {
    let mut b = [0u8; 8];
    // On failure (unlikely): mixed clock, like the old unsigned salt.
    if getrandom::fill(&mut b).is_err() {
        return now_ms().wrapping_mul(6_364_136_223_846_793_005).rotate_left(17);
    }
    i64::from_be_bytes(b)
}

fn random_u128() -> u128 {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    // UUID version 4.
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    u128::from_be_bytes(b)
}

/// Keys shared by all the connections of the session (like the game's manager) and
/// date of the last attempt.
struct KeyCache {
    keys: Option<Arc<ProfileKeys>>,
    last_try: Option<Instant>,
}

static KEYS: Mutex<KeyCache> = Mutex::new(KeyCache { keys: None, last_try: None });

/// Minimum delay between two key requests.
const RETRY: Duration = Duration::from_secs(3600);

/// Current pair, renewed if `refreshedAfter` has passed (at most one request per hour,
/// the first one right away). Failure: the previous pair is kept.
pub fn current_keys(token: &AccessToken) -> Option<Arc<ProfileKeys>> {
    let mut c = KEYS.lock().unwrap_or_else(|p| p.into_inner());
    let due = c.keys.as_ref().is_none_or(|k| k.due_refresh(now_ms()));
    let may_try = c.last_try.is_none_or(|t| t.elapsed() >= RETRY);
    if due && may_try {
        c.last_try = Some(Instant::now());
        match fetch(token) {
            Ok(k) => {
                authlog(format!(
                    "chat keys: received (expires in {} min)",
                    (k.expires_at_ms - now_ms()) / 60_000
                ));
                c.keys = Some(Arc::new(k));
            }
            Err(e) => authlog(format!("chat keys: failure ({e})")),
        }
    }
    c.keys.clone()
}

/// `POST player/certificates` (Bearer, empty body). Like the game (outside a development
/// environment), no pair is kept on disk: a `profilekeys/<uuid>.json` left
/// by another client is deleted at each request.
fn fetch(token: &AccessToken) -> Result<ProfileKeys, String> {
    if let Some(account) = crate::auth::account() {
        crate::session::forget_stored_profile_keys(account.uuid);
    }
    let url = crate::session::endpoint("player", "getCertificates").ok_or("services unavailable")?;
    authlog(format!("chat keys: request ({})", crate::session::host_of(&url)));
    let mut r = crate::session::agent()
        .post(&url)
        .header("Authorization", &format!("Bearer {}", token.expose()))
        .header("Content-Type", "application/json; charset=utf-8")
        .send_empty()
        .map_err(|e| crate::session::transport_kind(&e).to_owned())?;
    let status = r.status().as_u16();
    if status >= 400 {
        return Err(format!("HTTP {status}"));
    }
    let body = r.body_mut().with_config().limit(64 * 1024).read_to_string().map_err(|_| "unreadable body")?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|_| "unreadable JSON")?;
    ProfileKeys::from_json(&v)
}

/// Cache of the received signatures (128 slots, `chat.md` § 1.7).
pub struct SignatureCache {
    entries: Vec<Option<Box<Sig>>>,
}

impl Default for SignatureCache {
    fn default() -> Self {
        SignatureCache { entries: vec![None; 128] }
    }
}

impl SignatureCache {
    pub fn get(&self, index: u32) -> Option<&Sig> {
        self.entries.get(index as usize)?.as_deref()
    }

    /// Pushes the last seen of a message then its signature: they take the first
    /// slots (the most recent at 0), the rest shifts.
    pub fn push(&mut self, list: &[Sig]) {
        let mut queue: std::collections::VecDeque<Box<Sig>> = list.iter().map(|s| Box::new(*s)).collect();
        for slot in self.entries.iter_mut() {
            let Some(next) = queue.pop_back() else { break };
            if let Some(old) = slot.replace(next)
                && !list.iter().any(|s| s == &*old)
            {
                queue.push_front(old);
            }
        }
    }
}

/// Tracking of the last seen (`chat.md` § 2.4): 20 circular slots.
#[derive(Default)]
pub struct LastSeen {
    /// (signature, awaiting acknowledgement).
    slots: [Option<(Box<Sig>, bool)>; 20],
    tail: usize,
    offset: i32,
    last: Option<Box<Sig>>,
}

/// Update attached to a signed message or command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub offset: i32,
    pub acknowledged: [u8; 3],
    pub checksum: u8,
    pub signatures: Vec<Sig>,
}

impl LastSeen {
    /// Signed message processed; true if an acknowledgement (`chat_ack`) must go out.
    pub fn add(&mut self, sig: &Sig, shown: bool) -> bool {
        if self.last.as_deref() == Some(sig) {
            return false;
        }
        self.last = Some(Box::new(*sig));
        self.slots[self.tail] = shown.then(|| (Box::new(*sig), true));
        self.tail = (self.tail + 1) % 20;
        self.offset += 1;
        self.offset > 64
    }

    /// Deletion of a still-pending message: its slot is emptied.
    pub fn ignore_pending(&mut self, sig: &Sig) {
        if let Some(slot) = self.slots.iter_mut().find(|s| s.as_ref().is_some_and(|(s, p)| *p && **s == *sig)) {
            *slot = None;
        }
    }

    /// Offset to acknowledge (`chat_ack`), reset to 0; `None` if it is zero.
    pub fn take_ack(&mut self) -> Option<i32> {
        (self.offset > 0).then(|| std::mem::take(&mut self.offset))
    }

    /// Before each signed message or command: acknowledgement of the non-empty slots.
    pub fn update(&mut self) -> Update {
        let offset = std::mem::take(&mut self.offset);
        let mut acknowledged = [0u8; 3];
        let mut signatures = Vec::new();
        for i in 0..20 {
            if let Some((sig, pending)) = &mut self.slots[(self.tail + i) % 20] {
                acknowledged[i / 8] |= 1 << (i % 8);
                signatures.push(**sig);
                *pending = false;
            }
        }
        Update { offset, acknowledged, checksum: checksum(&signatures), signatures }
    }
}

/// Checksum of the last-seen list (0 reserved, replaced by 1).
pub fn checksum(sigs: &[Sig]) -> u8 {
    let h = sigs.iter().fold(1i32, |h, s| {
        let hs = s.iter().fold(1i32, |a, &b| a.wrapping_mul(31).wrapping_add(i32::from(b as i8)));
        h.wrapping_mul(31).wrapping_add(hs)
    });
    match h as u8 {
        0 => 1,
        b => b,
    }
}

/// Chat session: random identifier, pair, index of the signed messages.
pub struct ChatSession {
    pub id: u128,
    pub keys: Arc<ProfileKeys>,
    pub index: i32,
}

/// Signed content of a message or command argument (`authentication.md` § 3.4).
pub fn signed_content(
    player: u128,
    session: u128,
    index: i32,
    salt: i64,
    timestamp_ms: i64,
    content: &str,
    last_seen: &[Sig],
) -> Vec<u8> {
    let mut b = Vec::with_capacity(64 + content.len() + last_seen.len() * 256);
    b.extend_from_slice(&1i32.to_be_bytes());
    b.extend_from_slice(&player.to_be_bytes());
    b.extend_from_slice(&session.to_be_bytes());
    b.extend_from_slice(&index.to_be_bytes());
    b.extend_from_slice(&salt.to_be_bytes());
    b.extend_from_slice(&timestamp_ms.div_euclid(1000).to_be_bytes());
    b.extend_from_slice(&(content.len() as i32).to_be_bytes());
    b.extend_from_slice(content.as_bytes());
    b.extend_from_slice(&(last_seen.len() as i32).to_be_bytes());
    for s in last_seen {
        b.extend_from_slice(s);
    }
    b
}

impl ChatSession {
    pub fn new(keys: Arc<ProfileKeys>) -> ChatSession {
        ChatSession { id: random_u128(), keys, index: 0 }
    }

    /// Signature of the following content; consumes an index.
    pub fn sign(
        &mut self,
        player: u128,
        salt: i64,
        timestamp_ms: i64,
        content: &str,
        last_seen: &[Sig],
    ) -> Option<Sig> {
        let data = signed_content(player, self.id, self.index, salt, timestamp_ms, content, last_seen);
        self.index += 1;
        self.keys.sign(&data)
    }
}

/// Body of `chat_session_update`.
pub fn write_session_update(w: &mut Vec<u8>, s: &ChatSession) {
    w.extend_from_slice(&s.id.to_be_bytes());
    w.extend_from_slice(&s.keys.expires_at_ms.to_be_bytes());
    write_varint(w, s.keys.public_der.len() as i32);
    w.extend_from_slice(&s.keys.public_der);
    write_varint(w, s.keys.signature.len() as i32);
    w.extend_from_slice(&s.keys.signature);
}

fn write_update(w: &mut Vec<u8>, u: &Update) {
    write_varint(w, u.offset);
    w.extend_from_slice(&u.acknowledged);
    w.push(u.checksum);
}

/// Body of `chat`: optional signature, last seen.
pub fn write_chat(w: &mut Vec<u8>, message: &str, timestamp_ms: i64, salt: i64, sig: Option<&Sig>, u: &Update) {
    rc_proto::write_str(w, message);
    w.extend_from_slice(&timestamp_ms.to_be_bytes());
    w.extend_from_slice(&salt.to_be_bytes());
    w.push(u8::from(sig.is_some()));
    if let Some(s) = sig {
        w.extend_from_slice(s);
    }
    write_update(w, u);
}

/// Body of `chat_command_signed`: signatures of the signable arguments (empty list without
/// session), last seen.
pub fn write_command_signed(
    w: &mut Vec<u8>,
    command: &str,
    timestamp_ms: i64,
    salt: i64,
    sigs: &[(String, Sig)],
    u: &Update,
) {
    rc_proto::write_str(w, command);
    w.extend_from_slice(&timestamp_ms.to_be_bytes());
    w.extend_from_slice(&salt.to_be_bytes());
    write_varint(w, sigs.len() as i32);
    for (name, s) in sigs {
        rc_proto::write_str(w, name);
        w.extend_from_slice(s);
    }
    write_update(w, u);
}

/// Chat keys task of a game entry on an online server (`authentication.md`
/// § 3.2–3.3): obtains the pair, announces the session, renews it when `refreshedAfter` has
/// passed (at most one request per hour); stops at the end of the connection or at the next
/// game entry.
pub(crate) fn spawn_key_task(link: std::sync::Weak<crate::link::Link>, token: AccessToken, generation: u64) {
    let alive =
        move |link: &std::sync::Weak<crate::link::Link>| link.upgrade().filter(|l| l.key_task_alive(generation));
    let r = std::thread::Builder::new().name("chat keys".into()).spawn(move || {
        loop {
            {
                let Some(l) = alive(&link) else { return };
                if l.in_play()
                    && let Some(keys) = current_keys(&token)
                    && let Err(e) = l.start_chat_session(keys)
                {
                    authlog(format!("chat session: send failed ({e})"));
                }
            }
            for _ in 0..60 {
                std::thread::sleep(Duration::from_secs(1));
                if alive(&link).is_none() {
                    return;
                }
            }
        }
    });
    if let Err(e) = r {
        authlog(format!("chat keys: thread not started ({e})"));
    }
}

/// Chat state of a connection, shared between the network thread and the game.
#[derive(Default)]
pub struct ChatState {
    /// Profile UUID of the local player (signer).
    pub player: u128,
    /// Online server (`login` packet): keys requested.
    pub online: bool,
    /// The server enforces secure chat.
    pub enforced: bool,
    pub session: Option<ChatSession>,
    pub last_seen: LastSeen,
    pub cache: SignatureCache,
    /// Command tree received (`commands`).
    pub commands: Option<Arc<rc_commands::Tree>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso8601_ms("2026-10-12T00:00:00.5Z"), Some(1_791_763_200_500));
        assert_eq!(parse_iso8601_ms("2026-10-12T02:00:00.123456+02:00"), Some(1_791_763_200_123));
        assert_eq!(parse_iso8601_ms("not a date"), None);
    }

    fn sig(b: u8) -> Sig {
        [b; 256]
    }

    #[test]
    fn checksum_matches_java_hashes() {
        // Empty list: 1. An array of 256 zeros: Java array hash = 31^256 (mod 2^32).
        assert_eq!(checksum(&[]), 1);
        let zero = (0..256).fold(1i32, |a, _| a.wrapping_mul(31));
        assert_eq!(
            checksum(&[sig(0)]),
            match (31i32.wrapping_add(zero)) as u8 {
                0 => 1,
                b => b,
            }
        );
    }

    #[test]
    fn last_seen_window_and_ack() {
        let mut t = LastSeen::default();
        assert!(!t.add(&sig(1), true));
        assert!(!t.add(&sig(1), true)); // same signature: ignored
        assert!(!t.add(&sig(2), false)); // not displayed: empty slot
        let u = t.update();
        assert_eq!(u.offset, 2);
        assert_eq!(u.signatures, vec![sig(1)]);
        // Window: the oldest at bit 0; tail = 2, sig(1) in slot 0 → bit 18.
        assert_eq!(u.acknowledged, [0, 0, 1 << 2]);
        assert_eq!(t.update().offset, 0);
        for i in 0..70u8 {
            if t.add(&sig(10 + i), true) {
                assert_eq!(t.take_ack(), Some(65));
                return;
            }
        }
        panic!("acknowledgement never requested");
    }

    #[test]
    fn deleted_pending_message_is_not_acknowledged() {
        let mut t = LastSeen::default();
        t.add(&sig(5), true);
        t.ignore_pending(&sig(5));
        assert!(t.update().signatures.is_empty());
    }

    #[test]
    fn cache_push_moves_entries() {
        let mut c = SignatureCache::default();
        c.push(&[sig(1)]);
        c.push(&[sig(1), sig(2)]);
        // The most recent (last element pushed) in slot 0.
        assert_eq!(c.get(0), Some(&sig(2)));
        assert_eq!(c.get(1), Some(&sig(1)));
        c.push(&[sig(3)]);
        assert_eq!((c.get(0), c.get(1), c.get(2)), (Some(&sig(3)), Some(&sig(2)), Some(&sig(1))));
        assert_eq!(c.get(3), None);
    }

    #[test]
    fn keys_parse_sign_and_packets() {
        use rsa::pkcs1::EncodeRsaPublicKey;
        use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
        let mut rng = rand_core::UnwrapErr(getrandom::SysRng);
        let k = rsa::RsaPrivateKey::new(&mut rng, 2048).expect("key");
        let private = k.to_pkcs8_pem(LineEnding::LF).expect("pem").replace("PRIVATE KEY", "RSA PRIVATE KEY");
        let public =
            k.to_public_key().to_public_key_pem(LineEnding::LF).expect("pem").replace("PUBLIC KEY", "RSA PUBLIC KEY");
        let json = serde_json::json!({
            "keyPair": { "privateKey": private.as_str(), "publicKey": public },
            "publicKeySignature": "outdated",
            "publicKeySignatureV2": base64::engine::general_purpose::STANDARD.encode([9u8; 512]),
            "expiresAt": "2026-10-14T00:00:00Z",
            "refreshedAfter": "2026-10-13T16:00:00.000000Z",
        });
        let keys = Arc::new(ProfileKeys::from_json(&json).expect("keys"));
        assert!(!format!("{keys:?}").contains("PRIVATE"));
        assert_eq!(keys.signature.len(), 512);
        let mut s = ChatSession::new(keys.clone());
        let content = signed_content(7, s.id, 0, 42, 1_791_763_200_999, "hello", &[sig(3)]);
        assert_eq!(content.len(), 4 + 16 + 16 + 4 + 8 + 8 + 4 + 5 + 4 + 256);
        // version, player, session, index, salt, then the timestamp in seconds.
        assert_eq!(&content[40..48], &42i64.to_be_bytes());
        assert_eq!(&content[48..56], &1_791_763_200i64.to_be_bytes());
        let signature = s.sign(7, 42, 1_791_763_200_999, "hello", &[sig(3)]).expect("signature");
        assert_eq!(s.index, 1);
        let pkcs1 = k.to_public_key().to_pkcs1_der().expect("der");
        ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PKCS1_2048_8192_SHA256, pkcs1.as_bytes())
            .verify(&content, &signature)
            .expect("signature verified");
        // Packets.
        let mut w = Vec::new();
        write_session_update(&mut w, &s);
        assert_eq!(w.len(), 16 + 8 + 2 + keys.public_der.len() + 2 + 512);
        let u = Update { offset: 0, acknowledged: [0; 3], checksum: 1, signatures: vec![] };
        let mut w = Vec::new();
        write_chat(&mut w, "a", 5, 6, None, &u);
        assert_eq!(w, [1, b'a', 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 1]);
        let mut w = Vec::new();
        write_command_signed(&mut w, "say a", 5, 6, &[("message".into(), signature)], &u);
        assert_eq!(w.len(), 6 + 16 + 1 + 8 + 256 + 5);
    }
}
