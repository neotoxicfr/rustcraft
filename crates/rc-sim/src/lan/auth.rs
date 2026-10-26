//! Authenticated and encrypted login of LAN guests, like the game's integrated server
//! (which requires authentication): 1024-bit RSA key pair of the opened world, encryption
//! request with a 4-byte challenge, AES secret decrypted by the private key, then
//! `hasJoined` check with the session services ("verify" address of the discovery
//! document). A guest the services do not recognise, or unreachable services,
//! do not prevent entry: the game, in singleplayer, lets players in with the offline profile.

use std::sync::atomic::AtomicBool;

use rc_proto_v777::players::Property;

/// Authentication required from guests (and checked with the services). Disabled only for
/// trials with clients without an account (tests, `RUSTCRAFT_LAN_AUTH=0`): the encryption
/// request then says "not required", the link stays encrypted, the profile is offline.
pub static VERIFY: AtomicBool = AtomicBool::new(true);

/// Key pair of the opened world.
pub struct Keys {
    private: rsa::RsaPrivateKey,
    /// Public key, DER `SubjectPublicKeyInfo` (sent as is to the client).
    pub public_der: Vec<u8>,
}

impl Keys {
    /// New 1024-bit RSA pair (the game's size).
    pub fn generate() -> Result<Keys, String> {
        use rsa::pkcs8::EncodePublicKey;
        let mut rng = rand_core::UnwrapErr(getrandom::SysRng);
        let private = rsa::RsaPrivateKey::new(&mut rng, 1024).map_err(|e| format!("RSA key: {e}"))?;
        let public_der = rsa::RsaPublicKey::from(&private)
            .to_public_key_der()
            .map_err(|e| format!("public key: {e}"))?
            .as_bytes()
            .to_vec();
        Ok(Keys { private, public_der })
    }

    /// PKCS#1 v1.5 decryption by the private key (with random blinding).
    pub fn decrypt(&self, data: &[u8]) -> Option<Vec<u8>> {
        use rsa::traits::RandomizedDecryptor;
        let mut rng = rand_core::UnwrapErr(getrandom::SysRng);
        rsa::pkcs1v15::DecryptingKey::new(self.private.clone()).decrypt_with_rng(&mut rng, data).ok()
    }
}

/// Challenge of the encryption request: 4 random bytes (a big-endian integer in the game).
pub fn challenge() -> [u8; 4] {
    let mut c = [0u8; 4];
    let _ = getrandom::fill(&mut c);
    c
}

/// Body of the encryption request: empty server identifier, public key, challenge, and
/// the authentication requirement.
pub fn write_request(w: &mut Vec<u8>, public_der: &[u8], challenge: &[u8], authenticate: bool) {
    use rc_proto::Encode;
    rc_proto::write_str(w, "");
    for b in [public_der, challenge] {
        rc_proto::varint::write_varint(w, b.len() as i32);
        w.extend_from_slice(b);
    }
    authenticate.encode(w);
}

/// Client answer: shared secret and challenge, encrypted by the public key.
pub fn read_key(r: &mut rc_proto::Reader<'_>) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut take = || -> Option<Vec<u8>> {
        let n = rc_proto::read_count(r, 1).ok()?;
        Some(r.take(n).ok()?.to_vec())
    };
    let secret = take()?;
    let challenge = take()?;
    Some((secret, challenge))
}

/// AES secret and session digest from the answer; `None`: wrong challenge or unreadable
/// secret (protocol error, connection closed like the game).
pub fn accept_key(keys: &Keys, expected: &[u8], secret_enc: &[u8], challenge_enc: &[u8]) -> Option<([u8; 16], String)> {
    if keys.decrypt(challenge_enc)?.as_slice() != expected {
        return None;
    }
    let secret: [u8; 16] = keys.decrypt(secret_enc)?.try_into().ok()?;
    let digest = rc_client::session::server_hash("", &secret, &keys.public_der);
    Some((secret, digest))
}

/// Profile retained for a guest: name, UUID, properties (signed skin).
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub uuid: u128,
    pub properties: Vec<Property>,
}

/// Offline profile (UUID derived from the name, no properties).
pub fn offline(name: &str) -> Profile {
    Profile { name: name.to_owned(), uuid: rc_client::offline_uuid(name), properties: Vec::new() }
}

/// Decision of the integrated server after `hasJoined`: the verified profile (under the requested name), or the
/// offline profile if the services do not recognise it or do not answer.
pub fn verified_or_offline(name: &str, found: Option<rc_client::session::ProfileResult>) -> Profile {
    match found {
        Some(p) => Profile { name: name.to_owned(), uuid: p.id, properties: p.properties },
        None => offline(name),
    }
}

/// Verification address: `verify` of the session services, with `username` and `serverId`.
fn verify_url(base: &str, name: &str, digest: &str) -> String {
    let sep = if base.contains('?') { '&' } else { '?' };
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => (b as char).to_string(),
                b' ' => "+".to_owned(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    format!("{base}{sep}username={}&serverId={}", enc(name), enc(digest))
}

/// `hasJoined`: profile of the account that announced this digest, `None` otherwise (or services
/// unreachable, no address in the discovery document, unreadable answer).
pub fn has_joined(name: &str, digest: &str) -> Option<rc_client::session::ProfileResult> {
    let base = rc_client::session::endpoint("session", "verify")?;
    if !rc_client::session::trusted_service(&base) {
        return None;
    }
    let mut r = rc_client::session::agent().get(&verify_url(&base, name, digest)).call().ok()?;
    if r.status().as_u16() != 200 {
        return None;
    }
    let body = r.body_mut().with_config().limit(1 << 20).read_to_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    rc_client::session::parse_profile(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Complete exchange with the client side of rc-client: RSA encryption by the public key,
    /// challenge checked, secret recovered, identical digest on both sides.
    #[test]
    fn key_exchange() {
        let keys = Keys::generate().unwrap();
        let ch = challenge();
        let secret = rc_client::session::new_secret().unwrap();
        let s_enc = rc_client::session::rsa_encrypt(&keys.public_der, &secret).unwrap();
        let c_enc = rc_client::session::rsa_encrypt(&keys.public_der, &ch).unwrap();
        let (got, digest) = accept_key(&keys, &ch, &s_enc, &c_enc).expect("key accepted");
        assert_eq!(got, secret);
        assert_eq!(digest, rc_client::session::server_hash("", &secret, &keys.public_der));
        // Wrong challenge: refusal.
        let bad = rc_client::session::rsa_encrypt(&keys.public_der, &[1, 2, 3, 4]).unwrap();
        assert!(ch != [1, 2, 3, 4] && accept_key(&keys, &ch, &s_enc, &bad).is_none());
        // Body of the request re-read by the client.
        let mut w = Vec::new();
        write_request(&mut w, &keys.public_der, &ch, true);
        let req = rc_client::session::read_encryption_request(&mut rc_proto::Reader::new(&w)).unwrap();
        assert_eq!(
            (req.server_id.as_str(), req.public_key, req.challenge, req.authenticate),
            ("", keys.public_der, ch.to_vec(), true)
        );
    }

    /// Singleplayer: unrecognised guest → offline profile; recognised → UUID and skin of the account.
    #[test]
    fn verified_profile_else_offline() {
        assert_eq!(verified_or_offline("Guest", None), offline("Guest"));
        let p = rc_client::session::ProfileResult {
            id: 0x1234,
            name: "guest".into(),
            properties: vec![Property { name: "textures".into(), value: "v".into(), signature: Some("s".into()) }],
            forced_name_change: false,
            banned_skin: false,
        };
        let got = verified_or_offline("Guest", Some(p.clone()));
        assert_eq!((got.name.as_str(), got.uuid, got.properties), ("Guest", 0x1234, p.properties));
    }

    #[test]
    fn verification_address() {
        assert_eq!(
            verify_url("https://sessionserver.mojang.com/session/minecraft/hasJoined", "Guest_1", "-1f2e"),
            "https://sessionserver.mojang.com/session/minecraft/hasJoined?username=Guest_1&serverId=-1f2e"
        );
        assert_eq!(verify_url("https://h/v?a=1", "x", "0"), "https://h/v?a=1&username=x&serverId=0");
    }
}
