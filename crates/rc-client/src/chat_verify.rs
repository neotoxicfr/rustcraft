//! Verification of received signed chat (`docs/specs/chat.md` § 1.8): player chat
//! sessions (public key signed by the services), per-player validator (signature, chain,
//! expiry), like the game.
//!
//! - Services key set (`playerCertificateKeys`, RSA 4096 bits, SHA1withRSA) read once
//!   per launch (discovery address); empty if the services are unreachable:
//!   chat sessions are then ignored and secure chat is never enforced.
//! - Player without a session: messages accepted unsigned ("not secure"), or refused if the
//!   server enforces secure chat and the services keys are known.
//! - Player with a session: SHA256withRSA signature of the body verified, strictly
//!   increasing index; a single fault invalidates the chain until the next session.

use hashbrown::HashMap;
use std::sync::{Arc, Mutex};

use rc_proto_v777::play::PlayerChat;
use rc_proto_v777::players::{ChatSessionData, InfoUpdate, action};

use crate::signing::{Sig, signed_content};

/// Grace period after the expiry of a profile key (8 h).
const EXPIRY_GRACE_MS: i64 = 8 * 3_600_000;
/// A message received more than 7 min after its timestamp is no longer "secure".
pub const CLIENT_EXPIRY_MS: i64 = 7 * 60_000;

/// Encoded `rsaEncryption` OID (1.2.840.113549.1.1.1).
const RSA_OID: [u8; 9] = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];

/// DER reading: (tag, content, rest).
fn der(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (&l, mut b) = b.split_first()?;
    let len = if l < 0x80 {
        usize::from(l)
    } else {
        let n = usize::from(l & 0x7f);
        if n == 0 || n > 4 || b.len() < n {
            return None;
        }
        let v = b[..n].iter().fold(0usize, |a, &x| a << 8 | usize::from(x));
        b = &b[n..];
        v
    };
    (b.len() >= len).then(|| (tag, &b[..len], &b[len..]))
}

/// RSA `SubjectPublicKeyInfo` key (DER X.509) → `RSAPublicKey` (PKCS#1); `None` if it is not
/// a well-formed RSA key (the game then refuses the packet).
pub fn rsa_from_spki(spki: &[u8]) -> Option<&[u8]> {
    let (0x30, body, rest) = der(spki)? else { return None };
    if !rest.is_empty() {
        return None;
    }
    let (0x30, algo, body) = der(body)? else { return None };
    let (0x06, oid, _) = der(algo)? else { return None };
    if oid != RSA_OID {
        return None;
    }
    let (0x03, bits, _) = der(body)? else { return None };
    let (&0, key) = bits.split_first()? else { return None };
    let (0x30, _, rest) = der(key)? else { return None };
    rest.is_empty().then_some(key)
}

fn verify(alg: &'static ring::signature::RsaParameters, pkcs1: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    ring::signature::UnparsedPublicKey::new(alg, pkcs1).verify(msg, sig).is_ok()
}

/// State of the key set reading, like the game: only the first attempt makes callers wait;
/// after a failure, empty set and a new attempt in the background 5 min later (then 10, 20…
/// up to 320 min).
struct KeyState {
    keys: Option<Arc<Vec<Vec<u8>>>>,
    first_done: bool,
    failures: u32,
    retry_at: Option<std::time::Instant>,
}

static KEY_STATE: Mutex<KeyState> = Mutex::new(KeyState { keys: None, first_done: false, failures: 0, retry_at: None });

fn key_state() -> std::sync::MutexGuard<'static, KeyState> {
    KEY_STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Result of an attempt: keys kept, or next attempt scheduled.
fn record_attempt(s: &mut KeyState, keys: Option<Vec<Vec<u8>>>) {
    s.first_done = true;
    match keys {
        Some(k) => {
            s.keys = Some(Arc::new(k));
            s.retry_at = None;
        }
        None => {
            let minutes = 5u64 << s.failures.min(6);
            s.failures += 1;
            s.retry_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(minutes * 60));
        }
    }
}

/// Services keys for the profile keys (`playerCertificateKeys`), in PKCS#1.
pub fn services_keys() -> Arc<Vec<Vec<u8>>> {
    let mut s = key_state();
    if !s.first_done {
        // First attempt: the other callers wait for its end (lock held).
        let keys = fetch_services_keys();
        record_attempt(&mut s, keys);
    } else if s.keys.is_none() && s.retry_at.is_some_and(|t| std::time::Instant::now() >= t) {
        s.retry_at = None;
        let _ = std::thread::Builder::new().name("service keys".into()).spawn(|| {
            let keys = fetch_services_keys();
            record_attempt(&mut key_state(), keys);
        });
    }
    s.keys.clone().unwrap_or_default()
}

/// Reading of the key set (request without token, TLS verified).
fn fetch_services_keys() -> Option<Vec<Vec<u8>>> {
    use base64::Engine;
    let url = crate::session::endpoint("authentication", "getPublicKeys")?;
    let mut r = crate::session::agent().get(&url).call().ok()?;
    if r.status().as_u16() >= 400 {
        return None;
    }
    let body = r.body_mut().with_config().limit(1 << 20).read_to_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let keys = v["playerCertificateKeys"].as_array()?;
    // A malformed key invalidates the whole set, like the game ("malformed key data").
    keys.iter()
        .map(|k| {
            let der = base64::engine::general_purpose::STANDARD.decode(k["publicKey"].as_str()?).ok()?;
            rsa_from_spki(&der).map(<[u8]>::to_vec)
        })
        .collect()
}

/// Early reading of the key set (separate thread), so that it is ready on entering the game.
pub fn prefetch_services_keys() {
    let _ = std::thread::Builder::new().name("service keys".into()).spawn(|| {
        services_keys();
    });
}

/// Last accepted message of a chain (comparison "same message" and index).
#[derive(Clone, PartialEq)]
struct Last {
    index: i32,
    signature: Option<Sig>,
    timestamp_ms: i64,
    salt: i64,
    content: String,
}

enum Validator {
    /// No session: message accepted, signature removed.
    AcceptUnsigned,
    /// No session and secure chat enforced: everything is refused.
    RejectAll,
    Keyed {
        session: u128,
        expires_ms: i64,
        key: Vec<u8>,
        last: Option<Box<Last>>,
        chain_valid: bool,
    },
}

impl Validator {
    fn fallback(enforced: bool) -> Validator {
        if enforced { Validator::RejectAll } else { Validator::AcceptUnsigned }
    }
}

/// Outcome of the validation of a player message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Accepted; `signed`: the signature is kept (verified), otherwise removed.
    Accepted { signed: bool },
    /// Unknown sender or refused message: "validation error" line.
    Rejected,
}

/// Players known to the connection and their validator.
pub struct Remote {
    players: HashMap<u128, Validator>,
    server_enforced: bool,
    keys: Arc<Vec<Vec<u8>>>,
}

impl Remote {
    pub fn new(server_enforced: bool, keys: Arc<Vec<Vec<u8>>>) -> Remote {
        Remote { players: HashMap::new(), server_enforced, keys }
    }

    /// Secure chat enforced: the server requests it and the services keys are known.
    fn enforced(&self) -> bool {
        !self.keys.is_empty() && self.server_enforced
    }

    /// `player_info_update`: additions, then chat sessions. Error: unreadable public key
    /// (packet refused as by the game).
    pub fn apply(&mut self, u: &InfoUpdate) -> Result<(), String> {
        for e in &u.entries {
            if let Some(d) = &e.chat_key
                && rsa_from_spki(&d.key).is_none()
            {
                return Err("unreadable chat public key".into());
            }
        }
        let enforced = self.enforced();
        if u.actions & action::ADD_PLAYER != 0 {
            for e in &u.entries {
                self.players.entry(e.uuid).or_insert_with(|| Validator::fallback(enforced));
            }
        }
        if u.actions & action::INITIALIZE_CHAT != 0 {
            for e in &u.entries {
                if self.players.contains_key(&e.uuid) {
                    let v = self.session_validator(e.uuid, e.chat_key.as_ref(), enforced);
                    self.players.insert(e.uuid, v);
                }
            }
        }
        Ok(())
    }

    fn session_validator(&self, player: u128, data: Option<&ChatSessionData>, enforced: bool) -> Validator {
        let Some(d) = data.filter(|_| !self.keys.is_empty()) else { return Validator::fallback(enforced) };
        let mut payload = Vec::with_capacity(24 + d.key.len());
        payload.extend_from_slice(&player.to_be_bytes());
        payload.extend_from_slice(&d.expires_ms.to_be_bytes());
        payload.extend_from_slice(&d.key);
        let alg = &ring::signature::RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY;
        let signed = self.keys.iter().any(|k| verify(alg, k, &payload, &d.signature));
        match rsa_from_spki(&d.key) {
            Some(key) if signed => Validator::Keyed {
                session: d.session,
                expires_ms: d.expires_ms,
                key: key.to_vec(),
                last: None,
                chain_valid: true,
            },
            _ => Validator::fallback(enforced),
        }
    }

    pub fn remove(&mut self, players: &[u128]) {
        for p in players {
            self.players.remove(p);
        }
    }

    /// Validates a message; `last_seen`: last seen of the body, decompacted.
    pub fn validate(&mut self, m: &PlayerChat, last_seen: &[Sig], now_ms: i64) -> Verdict {
        let Some(v) = self.players.get_mut(&m.sender) else { return Verdict::Rejected };
        match v {
            Validator::AcceptUnsigned => Verdict::Accepted { signed: false },
            Validator::RejectAll => Verdict::Rejected,
            Validator::Keyed { session, expires_ms, key, last, chain_valid } => {
                let this = Last {
                    index: m.session_index,
                    signature: m.signature.as_deref().copied(),
                    timestamp_ms: m.timestamp_ms,
                    salt: m.salt,
                    content: m.content.clone(),
                };
                let ok = expires_ms.saturating_add(EXPIRY_GRACE_MS) >= now_ms
                    && this.signature.is_some_and(|sig| {
                        let body = signed_content(
                            m.sender,
                            *session,
                            m.session_index,
                            m.salt,
                            m.timestamp_ms,
                            &m.content,
                            last_seen,
                        );
                        verify(&ring::signature::RSA_PKCS1_2048_8192_SHA256, key, &body, &sig)
                    })
                    && match last {
                        Some(l) if **l != this => this.index > l.index,
                        _ => true,
                    };
                *chain_valid = *chain_valid && ok;
                if !*chain_valid {
                    return Verdict::Rejected;
                }
                *last = Some(Box::new(this));
                Verdict::Accepted { signed: true }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::EncodePublicKey;

    struct Keys {
        private: rsa::RsaPrivateKey,
        spki: Vec<u8>,
    }

    /// `DigestInfo` PKCS#1 v1.5 headers of SHA-1 and SHA-256.
    const SHA1_PREFIX: [u8; 15] =
        [0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14];
    const SHA256_PREFIX: [u8; 19] = [
        0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04,
        0x20,
    ];

    /// PKCS#1 v1.5 signature (digest computed by ring, header added by hand).
    fn rsa_sign(k: &rsa::RsaPrivateKey, sha1: bool, msg: &[u8]) -> Vec<u8> {
        let (alg, prefix): (_, &[u8]) = if sha1 {
            (&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &SHA1_PREFIX)
        } else {
            (&ring::digest::SHA256, &SHA256_PREFIX)
        };
        let mut info = prefix.to_vec();
        info.extend_from_slice(ring::digest::digest(alg, msg).as_ref());
        k.sign(rsa::Pkcs1v15Sign::new_unprefixed(), &info).expect("signature")
    }

    fn player_keys() -> Keys {
        let mut rng = rand_core::UnwrapErr(getrandom::SysRng);
        let private = rsa::RsaPrivateKey::new(&mut rng, 2048).expect("key");
        let spki = private.to_public_key().to_public_key_der().expect("der").as_bytes().to_vec();
        Keys { private, spki }
    }

    fn chat(sender: u128, index: i32, content: &str, sig: Option<Sig>) -> PlayerChat {
        PlayerChat {
            global_index: 0,
            sender,
            session_index: index,
            signature: sig.map(Box::new),
            content: content.into(),
            timestamp_ms: 1_000_000,
            salt: 42,
            last_seen: Vec::new(),
            unsigned: None,
            filter: (0, Vec::new()),
            chat_type: rc_proto_v777::play::ChatTypeRef::Registry(0),
            sender_name: None,
            target_name: None,
        }
    }

    fn sign(k: &Keys, sender: u128, session: u128, index: i32, content: &str) -> Sig {
        let body = signed_content(sender, session, index, 42, 1_000_000, content, &[]);
        rsa_sign(&k.private, false, &body).try_into().expect("256 bytes")
    }

    /// Player 7 with a session signed by a test "services key".
    fn setup(enforced: bool) -> (Remote, Keys) {
        let service = player_keys();
        let pk = player_keys();
        let player = 7u128;
        let mut payload = player.to_be_bytes().to_vec();
        payload.extend_from_slice(&i64::MAX.to_be_bytes());
        payload.extend_from_slice(&pk.spki);
        let key_sig = rsa_sign(&service.private, true, &payload);
        let services = vec![rsa_from_spki(&service.spki).unwrap().to_vec()];
        let mut r = Remote::new(enforced, Arc::new(services));
        let entry = rc_proto_v777::players::InfoEntry {
            uuid: player,
            chat_key: Some(ChatSessionData {
                session: 99,
                expires_ms: i64::MAX,
                key: pk.spki.clone(),
                signature: key_sig,
            }),
            ..Default::default()
        };
        r.apply(&InfoUpdate { actions: action::ADD_PLAYER | action::INITIALIZE_CHAT, entries: vec![entry] }).unwrap();
        (r, pk)
    }

    #[test]
    fn signed_chain_is_secure_until_first_fault() {
        let (mut r, k) = setup(true);
        let m0 = chat(7, 0, "hello", Some(sign(&k, 7, 99, 0, "hello")));
        assert_eq!(r.validate(&m0, &[], 0), Verdict::Accepted { signed: true });
        // Same message replayed: accepted (comparison with the last one).
        assert_eq!(r.validate(&m0, &[], 0), Verdict::Accepted { signed: true });
        // Altered content: wrong signature, chain invalidated for what follows.
        let bad = chat(7, 1, "hello!", Some(sign(&k, 7, 99, 1, "hello")));
        assert_eq!(r.validate(&bad, &[], 0), Verdict::Rejected);
        let m2 = chat(7, 2, "re", Some(sign(&k, 7, 99, 2, "re")));
        assert_eq!(r.validate(&m2, &[], 0), Verdict::Rejected);
    }

    #[test]
    fn index_must_increase_and_unsigned_is_refused_in_a_session() {
        let (mut r, k) = setup(false);
        let m1 = chat(7, 1, "a", Some(sign(&k, 7, 99, 1, "a")));
        assert_eq!(r.validate(&m1, &[], 0), Verdict::Accepted { signed: true });
        let m0 = chat(7, 0, "b", Some(sign(&k, 7, 99, 0, "b")));
        assert_eq!(r.validate(&m0, &[], 0), Verdict::Rejected);
        let (mut r, _) = setup(false);
        assert_eq!(r.validate(&chat(7, 0, "a", None), &[], 0), Verdict::Rejected);
    }

    #[test]
    fn players_without_session_follow_the_server_rule() {
        let (mut r, _) = setup(true);
        r.apply(&InfoUpdate {
            actions: action::ADD_PLAYER,
            entries: vec![rc_proto_v777::players::InfoEntry { uuid: 8, ..Default::default() }],
        })
        .unwrap();
        assert_eq!(r.validate(&chat(8, 0, "x", None), &[], 0), Verdict::Rejected);
        // Unknown sender.
        assert_eq!(r.validate(&chat(9, 0, "x", None), &[], 0), Verdict::Rejected);
        // Server without enforced secure chat, or unknown services keys: accepted unsigned.
        let mut open = Remote::new(true, Arc::new(Vec::new()));
        open.apply(&InfoUpdate {
            actions: action::ADD_PLAYER,
            entries: vec![rc_proto_v777::players::InfoEntry { uuid: 8, ..Default::default() }],
        })
        .unwrap();
        assert_eq!(open.validate(&chat(8, 0, "x", None), &[], 0), Verdict::Accepted { signed: false });
    }

    #[test]
    fn expired_key_and_bad_service_signature() {
        let (mut r, k) = setup(true);
        // Key expired for more than 8 h (expiry = i64::MAX here: never); we force a date.
        if let Some(Validator::Keyed { expires_ms, .. }) = r.players.get_mut(&7) {
            *expires_ms = 0;
        }
        let m = chat(7, 0, "a", Some(sign(&k, 7, 99, 0, "a")));
        assert_eq!(r.validate(&m, &[], EXPIRY_GRACE_MS + 1), Verdict::Rejected);
        // Session whose key is not signed by the services: fallback (refusal, enforced).
        let (mut r, _) = setup(true);
        let pk = player_keys();
        let entry = rc_proto_v777::players::InfoEntry {
            uuid: 7,
            chat_key: Some(ChatSessionData { session: 1, expires_ms: i64::MAX, key: pk.spki, signature: vec![0; 512] }),
            ..Default::default()
        };
        r.apply(&InfoUpdate { actions: action::INITIALIZE_CHAT, entries: vec![entry] }).unwrap();
        assert!(matches!(r.players.get(&7), Some(Validator::RejectAll)));
        // Unreadable key: packet refused.
        let entry = rc_proto_v777::players::InfoEntry {
            uuid: 7,
            chat_key: Some(ChatSessionData { key: vec![1, 2, 3], ..Default::default() }),
            ..Default::default()
        };
        assert!(r.apply(&InfoUpdate { actions: action::INITIALIZE_CHAT, entries: vec![entry] }).is_err());
    }
}
