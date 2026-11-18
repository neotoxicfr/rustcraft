//! Player skins (`docs/specs/entities.md` part 3, skin): `textures` property of the profile,
//! signature verified with the Minecraft services keys (except local player), download
//! (Mojang domains only) on a dedicated thread with disk cache, conversion of the 64 × 32 format.

use hashbrown::HashMap;
use std::path::PathBuf;

use base64::Engine;
use crossbeam_channel::{Receiver, Sender};
use rc_assets::texture::Image;

use crate::players::PlayerList;

/// Ready textures of a profile, each optional (like the game: a cape without a skin keeps the
/// default skin). Skin: processed 64 × 64 image, slim model, stable name (hash of the
/// texture); cape (64 × 32) and elytra: image and stable name.
pub struct Skin {
    pub skin: Option<(Image, bool, String)>,
    pub cape: Option<(Image, String)>,
    pub elytra: Option<(Image, String)>,
}

struct Job {
    uuid: u128,
    value: String,
    signature: Option<String>,
    local: bool,
}

pub struct Skins {
    tx: Sender<Job>,
    rx: Receiver<(u128, Skin)>,
    /// `textures` value already requested per player.
    asked: HashMap<u128, String>,
}

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

impl Skins {
    pub fn new(cache: PathBuf) -> Self {
        let (tx, jobs) = crossbeam_channel::unbounded::<Job>();
        let (done, rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("skins".into())
            .spawn(move || worker(&jobs, &done, &cache))
            .expect("skins thread");
        Skins { tx, rx, asked: HashMap::new() }
    }

    /// Forgets the values already requested (new game: players coming back with the
    /// same property must be requested again, the Tab list table having been emptied).
    pub fn forget_asked(&mut self) {
        self.asked.clear();
    }

    /// Requests the skins of players whose `textures` property is new.
    pub fn update(&mut self, players: &PlayerList, local: u128) {
        for (&uuid, p) in &players.map {
            let Some(t) = p.properties.iter().find(|t| t.name == "textures") else { continue };
            if self.asked.get(&uuid) == Some(&t.value) {
                continue;
            }
            self.asked.insert(uuid, t.value.clone());
            let _ = self.tx.send(Job {
                uuid,
                value: t.value.clone(),
                signature: t.signature.clone(),
                local: uuid == local,
            });
        }
    }

    /// Skin of a placed player head (`block-entities.md` § 4.5): `textures` property of the
    /// profile, with no signature check (the game accepts unsigned head skins);
    /// `key` = identity of the head.
    pub fn request_head(&mut self, key: u128, value: String) {
        if self.asked.get(&key) == Some(&value) {
            return;
        }
        self.asked.insert(key, value.clone());
        let _ = self.tx.send(Job { uuid: key, value, signature: None, local: true });
    }

    /// Skins arrived since the last call.
    pub fn poll(&self) -> impl Iterator<Item = (u128, Skin)> + '_ {
        self.rx.try_iter()
    }
}

/// HTTP client of the skins: headers and proxy of the game (`rc_client::session`). The game sets
/// no read timeout; one minute here, so that a blocked connection does not freeze the thread.
fn skin_agent() -> ureq::Agent {
    rc_client::session::http_agent(Some(std::time::Duration::from_secs(60)), true)
}

fn worker(jobs: &Receiver<Job>, done: &Sender<(u128, Skin)>, cache: &std::path::Path) {
    let agent = skin_agent();
    let mut keys: Option<Vec<Vec<u8>>> = None;
    for mut job in jobs {
        // Profile reduced to a username or an id (`name:…`, `id:…`): `textures` property
        // requested from the profile service, then processed like that of an ordinary head.
        if job.value.starts_with("name:") || job.value.starts_with("id:") {
            let Some(value) = resolve_profile(&agent, cache, &job.value) else {
                eprintln!("skin {:032x}: profile {} not found", job.uuid, job.value);
                continue;
            };
            job.value = value;
        }
        if !job.local {
            // Public keys of the profile properties (once, disk cache as a fallback).
            // Kept only if read: empty list (services not reachable yet) = new attempt.
            if keys.as_ref().is_none_or(Vec::is_empty) {
                keys = Some(profile_keys(&agent, cache));
            }
            let keys = keys.as_deref().unwrap_or_default();
            let ok = job.signature.as_ref().is_some_and(|s| verify(keys, job.value.as_bytes(), s));
            if !ok {
                eprintln!("skin {:032x}: signature missing or invalid ({} keys)", job.uuid, keys.len());
                continue;
            }
        }
        let Some(plan) = TexturePlan::of(&job.value) else { continue };
        // Prefixes reread on every check (discovery document, like the game): a
        // late discovery gives their effect back to the following skins.
        let prefixes = rc_client::session::texture_prefixes();
        // Like the game: a single address outside the allowed prefixes and the whole profile is refused.
        if !textures_allowed(&job.value, &prefixes) {
            eprintln!("skin {:032x}: address refused", job.uuid);
            continue;
        }
        // Skin, cape and elytra independent: without a skin, the game keeps the default skin and
        // still applies the cape and the elytra.
        let mut skin = None;
        if let Some((url, slim)) = &plan.skin {
            let Some(key) = texture_key(url) else { continue };
            let Some(bytes) = fetch(&agent, url, &cache.join("skins").join(key)) else {
                eprintln!("skin {:032x}: download failed ({url})", job.uuid);
                continue;
            };
            let Some(image) = Image::decode_png(&bytes).and_then(process_legacy) else {
                eprintln!("skin {:032x}: image refused", job.uuid);
                continue;
            };
            skin = Some((image, *slim, format!("skin/{key}")));
        }
        let extra = |url: &Option<String>, prefix: &str| {
            let url = url.as_deref()?;
            let k = texture_key(url)?;
            let img = Image::decode_png(&fetch(&agent, url, &cache.join("skins").join(k))?)?;
            Some((img, format!("{prefix}/{k}")))
        };
        let (cape, elytra) = (extra(&plan.cape, "cape"), extra(&plan.elytra, "elytra"));
        let _ = done.send((job.uuid, Skin { skin, cape, elytra }));
    }
}

/// File name of a texture address (last segment, non-empty alphanumeric).
fn texture_key(url: &str) -> Option<&str> {
    let k = url.rsplit('/').next()?;
    (!k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(k)
}

/// Textures announced by the `textures` property: skin (and slim model), cape, elytra, each
/// optional. `None`: unreadable value or without any texture.
#[derive(Debug, PartialEq)]
struct TexturePlan {
    skin: Option<(String, bool)>,
    cape: Option<String>,
    elytra: Option<String>,
}

impl TexturePlan {
    fn of(value: &str) -> Option<Self> {
        let json: serde_json::Value = serde_json::from_slice(&B64.decode(value).ok()?).ok()?;
        let t = &json["textures"];
        let url = |k: &str| t[k]["url"].as_str().map(str::to_owned);
        let skin = url("SKIN").map(|u| (u, t["SKIN"]["metadata"]["model"].as_str() == Some("slim")));
        let plan = TexturePlan { skin, cape: url("CAPE"), elytra: url("ELYTRA") };
        (plan.skin.is_some() || plan.cape.is_some() || plan.elytra.is_some()).then_some(plan)
    }
}

/// Validity duration of a profile kept on disk (one day); expired, it still serves if the
/// service is unreachable.
const PROFILE_TTL: std::time::Duration = std::time::Duration::from_secs(86_400);

/// `textures` property (base 64) of the profile of a username (`name:Username`) or an id
/// (`id:<32 hexadecimals>`), through the Mojang profile services, with disk cache.
fn resolve_profile(agent: &ureq::Agent, cache: &std::path::Path, spec: &str) -> Option<String> {
    let file_name: String =
        spec.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ':').collect::<String>().to_lowercase();
    if file_name.len() < 4 {
        return None;
    }
    let file = cache.join("skins").join(format!("profile-{}", file_name.replace(':', "-")));
    let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if fresh.is_some_and(|age| age < PROFILE_TTL)
        && let Ok(v) = std::fs::read_to_string(&file)
    {
        return Some(v);
    }
    let get = |url: &str| -> Option<serde_json::Value> {
        serde_json::from_str(&agent.get(url).call().ok()?.body_mut().read_to_string().ok()?).ok()
    };
    // Like the game's profile resolver: username (lowercase) → id through the profile
    // service, then signed profile (`unsigned=false`) through the session services, addresses
    // from the discovery document.
    let fetched = (|| {
        let id = match spec.strip_prefix("id:") {
            Some(id) => id.to_owned(),
            None => {
                let name = spec.strip_prefix("name:")?;
                if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                    return None;
                }
                let by_name = rc_client::session::endpoint("profiles", "getByName")?;
                get(&by_name.replace("{name}", &name.to_ascii_lowercase()))?["id"].as_str()?.to_owned()
            }
        };
        let id = rc_client::auth::parse_uuid(&id)?;
        let p = rc_client::session::fetch_profile(id, true)?;
        Some(p.properties.into_iter().find(|t| t.name == "textures")?.value)
    })();
    match fetched {
        Some(v) => {
            let _ = std::fs::create_dir_all(file.parent()?);
            let _ = std::fs::write(&file, &v);
            Some(v)
        }
        // Service unreachable: stale copy.
        None => std::fs::read_to_string(&file).ok(),
    }
}

/// Test: signed `textures` property of the public profile of `name` (Mojang API), given to the
/// players that have none (except the local player).
pub fn apply_test_profile(players: &mut PlayerList, name: &str, local: u128) {
    static PROFILE: std::sync::OnceLock<Option<rc_proto_v777::players::Property>> = std::sync::OnceLock::new();
    let prop = PROFILE.get_or_init(|| {
        let by_name = rc_client::session::endpoint("profiles", "getByName")?;
        let mut r = skin_agent().get(&by_name.replace("{name}", &name.to_ascii_lowercase())).call().ok()?;
        let v: serde_json::Value = serde_json::from_str(&r.body_mut().read_to_string().ok()?).ok()?;
        let id = rc_client::auth::parse_uuid(v["id"].as_str()?)?;
        rc_client::session::fetch_profile(id, true)?.properties.into_iter().find(|t| t.name == "textures")
    });
    let Some(prop) = prop else {
        eprintln!("test skin: profile of {name} not found");
        return;
    };
    eprintln!("test skin: profile of {name} applied");
    for (&uuid, p) in &mut players.map {
        if uuid != local && !p.properties.iter().any(|t| t.name == "textures") {
            p.properties.push(prop.clone());
        }
    }
}

/// Texture address allowed, like the game: it starts with one of the prefixes of the
/// discovery document (`profiles`/`getTexture`, `validUris`); no prefix (services unavailable):
/// nothing is allowed.
fn allowed(url: &str, prefixes: &[String]) -> bool {
    prefixes.iter().any(|p| !p.is_empty() && url.starts_with(p.as_str()))
}

/// All the textures of the property (skin, cape, elytra) have an allowed address.
fn textures_allowed(value: &str, prefixes: &[String]) -> bool {
    let Some(json) = B64.decode(value).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
        return false;
    };
    let Some(textures) = json["textures"].as_object() else { return false };
    textures.values().all(|t| t["url"].as_str().is_some_and(|u| allowed(u, prefixes)))
}

fn fetch(agent: &ureq::Agent, url: &str, local: &std::path::Path) -> Option<Vec<u8>> {
    if let Ok(b) = std::fs::read(local) {
        return Some(b);
    }
    let mut resp = agent.get(url).call().ok()?;
    let bytes = resp.body_mut().with_config().limit(1 << 20).read_to_vec().ok()?;
    if let Some(dir) = local.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(local, &bytes);
    }
    Some(bytes)
}

/// RSA keys (PKCS#1, DER) of the profile properties: Minecraft services, otherwise disk cache.
fn profile_keys(agent: &ureq::Agent, cache: &std::path::Path) -> Vec<Vec<u8>> {
    let file = cache.join("skins").join("publickeys.json");
    let body = rc_client::session::endpoint("authentication", "getPublicKeys")
        .and_then(|url| agent.get(&url).call().ok())
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .inspect(|b| {
            let _ = std::fs::create_dir_all(file.parent().expect("folder"));
            let _ = std::fs::write(&file, b);
        })
        .or_else(|| std::fs::read_to_string(&file).ok())
        .unwrap_or_default();
    let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    json["profilePropertyKeys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| B64.decode(k["publicKey"].as_str()?).ok())
        .filter_map(|spki| rsa_from_spki(&spki).map(<[u8]>::to_vec))
        .collect()
}

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

/// PKCS#1 RSA key contained in a `SubjectPublicKeyInfo`.
fn rsa_from_spki(spki: &[u8]) -> Option<&[u8]> {
    let (0x30, body, _) = der(spki)? else { return None };
    let (0x30, _alg, rest) = der(body)? else { return None };
    let (0x03, bits, _) = der(rest)? else { return None };
    // First byte of the bit string: unused bits (0).
    bits.split_first().filter(|(unused, _)| **unused == 0).map(|(_, k)| k)
}

fn verify(keys: &[Vec<u8>], data: &[u8], signature: &str) -> bool {
    let Ok(sig) = B64.decode(signature) else { return false };
    keys.iter().any(|k| {
        ring::signature::UnparsedPublicKey::new(&ring::signature::RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY, k)
            .verify(data, &sig)
            .is_ok()
    })
}

/// Processing of a downloaded skin: 64 × 64 or 64 × 32 (enlarged, left limbs copied
/// mirrored), base zones opaque, fully opaque hat of an old format cleared.
fn process_legacy(mut img: Image) -> Option<Image> {
    if img.width != 64 || !(img.height == 32 || img.height == 64) {
        return None;
    }
    let legacy = img.height == 32;
    if legacy {
        img.rgba.resize(64 * 64 * 4, 0);
        img.height = 64;
        // (x, y, dx, dy, width, height): horizontally flipped copy.
        const COPIES: [(i32, i32, i32, i32, i32, i32); 12] = [
            (4, 16, 16, 32, 4, 4),
            (8, 16, 16, 32, 4, 4),
            (0, 20, 24, 32, 4, 12),
            (4, 20, 16, 32, 4, 12),
            (8, 20, 8, 32, 4, 12),
            (12, 20, 16, 32, 4, 12),
            (44, 16, -8, 32, 4, 4),
            (48, 16, -8, 32, 4, 4),
            (40, 20, 0, 32, 4, 12),
            (44, 20, -8, 32, 4, 12),
            (48, 20, -16, 32, 4, 12),
            (52, 20, -8, 32, 4, 12),
        ];
        for (x, y, dx, dy, w, h) in COPIES {
            for j in 0..h {
                for i in 0..w {
                    let src = (((y + j) * 64 + x + i) * 4) as usize;
                    let dst = (((y + dy + j) * 64 + x + dx + w - 1 - i) * 4) as usize;
                    let p: [u8; 4] = img.rgba[src..src + 4].try_into().expect("pixel");
                    img.rgba[dst..dst + 4].copy_from_slice(&p);
                }
            }
        }
    }
    let mut opaque = |x0: usize, y0: usize, x1: usize, y1: usize| {
        for y in y0..y1 {
            for x in x0..x1 {
                img.rgba[(y * 64 + x) * 4 + 3] = 255;
            }
        }
    };
    opaque(0, 0, 32, 16);
    if legacy {
        let all_opaque = (0..32).all(|y| (32..64).all(|x| img.rgba[(y * 64 + x) * 4 + 3] >= 128));
        if all_opaque {
            for y in 0..32 {
                for x in 32..64 {
                    img.rgba[(y * 64 + x) * 4 + 3] = 0;
                }
            }
        }
    }
    let mut opaque = |x0: usize, y0: usize, x1: usize, y1: usize| {
        for y in y0..y1 {
            for x in x0..x1 {
                img.rgba[(y * 64 + x) * 4 + 3] = 255;
            }
        }
    };
    opaque(0, 16, 64, 32);
    opaque(16, 48, 48, 64);
    Some(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_domains() {
        // Prefixes of the discovery document recorded on 18/10/2026.
        let p = vec![
            "https://textures.minecraft.net/texture/".to_owned(),
            "http://textures.minecraft.net/texture/".to_owned(),
        ];
        assert!(allowed("http://textures.minecraft.net/texture/abc123", &p));
        assert!(allowed("https://textures.minecraft.net/texture/abc123", &p));
        assert!(!allowed("http://bugs.mojang.com/x", &p));
        assert!(!allowed("http://evil.example/texture", &p));
        assert!(!allowed("http://minecraft.net.evil.example/texture", &p));
        assert!(!allowed("file:///etc/passwd", &p));
        // Difference fixed: another Mojang subdomain is not a game texture address.
        assert!(!allowed("https://launcher.mojang.com/texture/abc", &p));
        assert!(!allowed("https://textures.minecraft.net/other/abc", &p));
        // Services unavailable: nothing is allowed.
        assert!(!allowed("https://textures.minecraft.net/texture/abc123", &[]));
    }

    #[test]
    fn one_foreign_texture_rejects_the_whole_profile() {
        let p = vec!["https://textures.minecraft.net/texture/".to_owned()];
        let enc = |j: &str| B64.encode(j);
        let ok = enc(r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a1"}}}"#);
        assert!(textures_allowed(&ok, &p));
        let bad = enc(
            r#"{"textures":{"SKIN":{"url":"https://textures.minecraft.net/texture/a1"},"CAPE":{"url":"https://evil.example/c"}}}"#,
        );
        assert!(!textures_allowed(&bad, &p));
        assert!(!textures_allowed("not base64", &p));
    }

    /// Skin, cape and elytra read independently: a profile without a skin keeps its cape and its
    /// elytra (the game then sets the default skin); profile without a texture: nothing to do.
    #[test]
    fn cape_and_elytra_do_not_need_a_skin() {
        let enc = |j: &str| B64.encode(j);
        let p =
            TexturePlan::of(&enc(r#"{"textures":{"CAPE":{"url":"https://t/c1"},"ELYTRA":{"url":"https://t/e1"}}}"#));
        assert_eq!(
            p,
            Some(TexturePlan { skin: None, cape: Some("https://t/c1".into()), elytra: Some("https://t/e1".into()) })
        );
        let p = TexturePlan::of(&enc(r#"{"textures":{"SKIN":{"url":"https://t/s1","metadata":{"model":"slim"}}}}"#));
        assert_eq!(p, Some(TexturePlan { skin: Some(("https://t/s1".into(), true)), cape: None, elytra: None }));
        assert_eq!(TexturePlan::of(&enc(r#"{"textures":{}}"#)), None);
        assert_eq!(texture_key("https://t/texture/ab12"), Some("ab12"));
        assert_eq!(texture_key("https://t/texture/a.png"), None);
        assert_eq!(texture_key("https://t/texture/"), None);
    }

    #[test]
    fn legacy_skin_mirrors_right_leg_to_left() {
        let mut rgba = vec![0u8; 64 * 32 * 4];
        // Distinctive pixel at the top left of the front face of the right leg (4, 20).
        let i = (20 * 64 + 4) * 4;
        rgba[i..i + 4].copy_from_slice(&[10, 20, 30, 255]);
        let img = process_legacy(Image { width: 64, height: 32, rgba }).unwrap();
        assert_eq!(img.height, 64);
        // Flipped copy: (4, 20) → x = 4 + 16 + 4 − 1 − 0 = 23, y = 52.
        let j = (52 * 64 + 23) * 4;
        assert_eq!(&img.rgba[j..j + 4], &[10, 20, 30, 255]);
    }

    #[test]
    fn spki_unwraps_rsa_key() {
        // SEQUENCE { SEQUENCE {}, BIT STRING { 00, 30 00 } }
        let spki = [0x30, 0x07, 0x30, 0x00, 0x03, 0x03, 0x00, 0x30, 0x00];
        assert_eq!(rsa_from_spki(&spki), Some(&[0x30, 0x00][..]));
    }
}
