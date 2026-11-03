//! Accuracy packets (`docs/specs/protocol-supplements.md`): decoding on the network thread,
//! immediate service replies (cookies, resource packs) and events for the game.

use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{Decode, Encode, Reader};
use rc_proto_v777::chunk::ChunkContext;
use rc_proto_v777::entity::{MetaCodec, Particle};
use rc_proto_v777::play::{SoundRef, read_sound_ref};
use rc_text::Component;

use crate::link::Link;
use crate::{Error, Event};

/// `explode` (§ 3).
#[derive(Debug, Clone, PartialEq)]
pub struct Explosion {
    pub center: [f64; 3],
    pub radius: f32,
    pub blocks: i32,
    pub knockback: Option<[f64; 3]>,
    pub particle: Particle,
    pub sound: SoundRef,
    /// Block particles: (particle, scale, speed, weight).
    pub block_particles: Vec<(Particle, f32, f32, i32)>,
    pub play_sound: bool,
}

/// Server link (§ 11): known type (0 to 9) or free-form label.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerLink {
    pub kind: Option<i32>,
    pub label: Option<Component>,
    pub url: String,
}

/// Dialog to display (§ 12): entry of the `minecraft:dialog` registry or direct content.
#[derive(Debug, Clone, PartialEq)]
pub enum DialogRef {
    Registry(i32),
    Inline(rc_proto::nbt::Nbt),
}

/// What the network thread does with a packet of this module.
pub(crate) enum Handled {
    /// Handled (reply possibly already sent).
    Done,
    Event(Event),
    /// `transfer`: last event of the session.
    Transfer(Event),
}

/// Maximum size of a cookie.
const COOKIE_MAX: usize = 5120;

fn read_bytes<'a>(r: &mut Reader<'a>, max: usize) -> rc_proto::Result<&'a [u8]> {
    let n = rc_proto::read_len(r, max)?;
    r.take(n)
}

fn str_owned(r: &mut Reader<'_>, max: usize) -> rc_proto::Result<String> {
    rc_proto::read_str(r, max).map(str::to_owned)
}

/// Cookies and resource packs: present in the login (cookies), configuration and play phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommonPacket {
    CookieRequest,
    StoreCookie,
    PackPush,
    PackPop,
}

/// Identifiers of the replies to the common packets, in one phase (-1: absent from it).
pub(crate) struct CommonReplies {
    pub cookie_response: i32,
    pub pack_out: i32,
}

/// Packet `id` of the cookies or resource packs.
pub(crate) fn common(
    link: &Link,
    replies: &CommonReplies,
    packet: CommonPacket,
    id: i32,
    r: &mut Reader<'_>,
) -> Result<Handled, Error> {
    // Detail text built only when the packet log is on.
    let detail = |text: &dyn Fn() -> String| link.names.detail_with(rc_data::Bound::ToClient, id, text);
    match packet {
        CommonPacket::CookieRequest => {
            let key = str_owned(r, 32767)?;
            let value = link.cookies.lock().unwrap_or_else(|p| p.into_inner()).get(&key).cloned();
            detail(&|| format!("key {key}, known: {}", value.is_some()));
            link.send(replies.cookie_response, |w| {
                rc_proto::write_str(w, &key);
                value.is_some().encode(w);
                if let Some(v) = &value {
                    write_varint(w, v.len() as i32);
                    w.extend_from_slice(v);
                }
            })?;
            Ok(Handled::Done)
        }
        CommonPacket::StoreCookie => {
            let key = str_owned(r, 32767)?;
            let value = read_bytes(r, COOKIE_MAX)?.to_vec();
            detail(&|| format!("key {key}, {} B", value.len()));
            link.cookies.lock().unwrap_or_else(|p| p.into_inner()).insert(key, value);
            Ok(Handled::Done)
        }
        CommonPacket::PackPush => {
            let uuid = u128::decode(r)?;
            let url = str_owned(r, 32767)?;
            let hash = str_owned(r, 40)?;
            let required = bool::decode(r)?;
            let prompt = if bool::decode(r)? { Some(Box::new(read_text(r)?)) } else { None };
            // Invalid URL: 5 right away, nothing else; otherwise the game decides (prompt, preference).
            if !valid_url(&url) {
                detail(&|| format!("pack {url}: invalid URL → 5"));
                link.send(replies.pack_out, |w| {
                    uuid.encode(w);
                    write_varint(w, 5);
                })?;
                return Ok(Handled::Done);
            }
            detail(&|| format!("pack {url}, required {required}"));
            Ok(Handled::Event(Event::PackPush { id: uuid, url, hash, required, prompt }))
        }
        CommonPacket::PackPop => {
            // No reply for a requested removal.
            let which = if bool::decode(r)? { Some(u128::decode(r)?) } else { None };
            Ok(Handled::Event(Event::PackPop(which)))
        }
    }
}

/// Acceptable pack URL: http or https scheme and non-empty host.
fn valid_url(url: &str) -> bool {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://"));
    rest.is_some_and(|r| !r.is_empty() && !r.starts_with('/') && !r.contains(char::is_whitespace))
}

fn read_text(r: &mut Reader<'_>) -> rc_proto::Result<Component> {
    Ok(rc_proto::nbt::read(r)?.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| Component::text("")))
}

fn read_links(r: &mut Reader<'_>) -> rc_proto::Result<Vec<ServerLink>> {
    let n = rc_proto::read_count(r, 2)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let (kind, label) = if bool::decode(r)? {
            let k = read_varint(r)?;
            (Some(if (0..=9).contains(&k) { k } else { 0 }), None)
        } else {
            (None, Some(read_text(r)?))
        };
        let url = str_owned(r, 32767)?;
        if valid_url(&url) {
            out.push(ServerLink { kind, label, url });
        } else {
            eprintln!("server link ignored (scheme): {url}");
        }
    }
    Ok(out)
}

fn read_report(r: &mut Reader<'_>) -> rc_proto::Result<Vec<(String, String)>> {
    let n = rc_proto::read_count(r, 2)?;
    if n > 32 {
        return Err(rc_proto::Error::LengthOutOfRange { len: n as i64, max: 32 });
    }
    (0..n).map(|_| Ok((str_owned(r, 128)?, str_owned(r, 4096)?))).collect()
}

fn read_transfer(r: &mut Reader<'_>) -> rc_proto::Result<Event> {
    let host = str_owned(r, 32767)?;
    let port = read_varint(r)?;
    Ok(Event::Transfer { host, port: port.clamp(0, 65535) as u16 })
}

/// Configuration packets of this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigExtraPacket {
    Transfer,
    Features,
    ResetChat,
    Report,
    Links,
    ShowDialog,
    ClearDialog,
}

pub(crate) fn config(packet: ConfigExtraPacket, r: &mut Reader<'_>) -> Result<Handled, Error> {
    use ConfigExtraPacket as P;
    Ok(match packet {
        P::Transfer => Handled::Transfer(read_transfer(r)?),
        P::Features => {
            let n = rc_proto::read_count(r, 1)?;
            let flags = (0..n).map(|_| str_owned(r, 32767)).collect::<rc_proto::Result<_>>()?;
            Handled::Event(Event::EnabledFeatures(flags))
        }
        P::ResetChat => Handled::Event(Event::ResetChat),
        P::Report => Handled::Event(Event::ReportDetails(read_report(r)?)),
        P::Links => Handled::Event(Event::ServerLinks(read_links(r)?)),
        P::ShowDialog => {
            let nbt = rc_proto::nbt::read(r)?.ok_or_else(|| Error::Invalid("empty dialog".into()))?;
            Handled::Event(Event::ShowDialog(DialogRef::Inline(nbt)))
        }
        P::ClearDialog => Handled::Event(Event::ClearDialog),
    })
}

/// Play packets of this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayExtraPacket {
    TickingState,
    TickingStep,
    Explode,
    Rotation,
    LookAt,
    Camera,
    Vehicle,
    Transfer,
    Report,
    Links,
    ShowDialog,
    ClearDialog,
    Biomes,
    Simulation,
    Difficulty,
    GameRules,
}

fn vec3(r: &mut Reader<'_>) -> rc_proto::Result<[f64; 3]> {
    Ok([f64::decode(r)?, f64::decode(r)?, f64::decode(r)?])
}

pub(crate) fn play(
    link: &Link,
    packet: PlayExtraPacket,
    r: &mut Reader<'_>,
    meta: &MetaCodec,
    ctx: Option<&ChunkContext>,
) -> Result<Handled, Error> {
    let ev = match packet {
        PlayExtraPacket::TickingState => {
            // F3 readings (`netstats.rs`) read on a copy, then the state for the game's tick rate.
            link.stats.ticking_state(&mut Reader::new(r.rest()))?;
            Event::TickingState { rate: f32::decode(r)?, frozen: bool::decode(r)? }
        }
        PlayExtraPacket::TickingStep => {
            link.stats.ticking_step(&mut Reader::new(r.rest()))?;
            Event::TickingStep(read_varint(r)?)
        }
        PlayExtraPacket::Explode => explosion(r, meta)?,
        PlayExtraPacket::Rotation => {
            let yaw = f32::decode(r)?;
            let yaw_relative = bool::decode(r)?;
            let pitch = f32::decode(r)?;
            let pitch_relative = bool::decode(r)?;
            Event::PlayerRotation { yaw, yaw_relative, pitch, pitch_relative }
        }
        PlayExtraPacket::LookAt => {
            let eyes = read_varint(r)? == 1;
            let target = vec3(r)?;
            let entity = if bool::decode(r)? { Some((read_varint(r)?, read_varint(r)? == 1)) } else { None };
            Event::LookAt { eyes, target, entity }
        }
        PlayExtraPacket::Camera => Event::SetCamera(read_varint(r)?),
        PlayExtraPacket::Vehicle => {
            let pos = vec3(r)?;
            Event::MoveVehicle { pos, yaw: f32::decode(r)?, pitch: f32::decode(r)? }
        }
        PlayExtraPacket::Transfer => return Ok(Handled::Transfer(read_transfer(r)?)),
        PlayExtraPacket::Report => Event::ReportDetails(read_report(r)?),
        PlayExtraPacket::Links => Event::ServerLinks(read_links(r)?),
        PlayExtraPacket::ShowDialog => {
            let k = read_varint(r)?;
            if k > 0 {
                Event::ShowDialog(DialogRef::Registry(k - 1))
            } else {
                let nbt = rc_proto::nbt::read(r)?.ok_or_else(|| Error::Invalid("empty dialog".into()))?;
                Event::ShowDialog(DialogRef::Inline(nbt))
            }
        }
        PlayExtraPacket::ClearDialog => Event::ClearDialog,
        PlayExtraPacket::Biomes => {
            let Some(ctx) = ctx else { return Ok(Handled::Done) };
            Event::ChunkBiomes(rc_proto_v777::chunk::read_chunks_biomes(r, ctx)?)
        }
        PlayExtraPacket::Simulation => Event::SimulationDistance(read_varint(r)?),
        PlayExtraPacket::Difficulty => {
            let d = read_varint(r)?;
            Event::Difficulty { id: if (0..=3).contains(&d) { d } else { 0 }, locked: bool::decode(r)? }
        }
        PlayExtraPacket::GameRules => {
            // Reply to the rules request: for the "Game rules" screen (`world_options.rs`).
            let n = rc_proto::read_count(r, 2)?;
            let mut rules = Vec::with_capacity(n);
            for _ in 0..n {
                rules.push((rc_proto::read_str(r, 32767)?.to_owned(), rc_proto::read_str(r, 32767)?.to_owned()));
            }
            Event::GameRuleValues(rules)
        }
    };
    Ok(Handled::Event(ev))
}

/// `explode` (§ 3).
fn explosion(r: &mut Reader<'_>, meta: &MetaCodec) -> rc_proto::Result<Event> {
    let center = vec3(r)?;
    let radius = f32::decode(r)?;
    let blocks = i32::decode(r)?;
    let knockback = if bool::decode(r)? { Some(vec3(r)?) } else { None };
    let particle = meta.particle(r)?;
    let sound = read_sound_ref(r)?;
    let n = rc_proto::read_count(r, 4)?;
    let mut block_particles = Vec::with_capacity(n);
    for _ in 0..n {
        let p = meta.particle(r)?;
        block_particles.push((p, f32::decode(r)?, f32::decode(r)?, read_varint(r)?));
    }
    let play_sound = bool::decode(r)?;
    Ok(Event::Explode(Box::new(Explosion {
        center,
        radius,
        blocks,
        knockback,
        particle,
        sound,
        block_particles,
        play_sound,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_urls() {
        assert!(valid_url("http://a.b/p.zip"));
        assert!(valid_url("https://x"));
        assert!(!valid_url("ftp://x"));
        assert!(!valid_url("http://"));
        assert!(!valid_url("whatever"));
    }
}
