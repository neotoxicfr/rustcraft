//! Light play packets (position, blocks, forgotten chunks) and packets sent by the client.
//! Format: `docs/specs/connection.md`.

use rc_proto::varint::{read_varint, read_varlong, write_varint};
use rc_proto::{BlockPos, Decode, Encode, Reader, Result, write_str};

/// Relative flags of the positioning.
pub mod relative {
    pub const X: i32 = 1;
    pub const Y: i32 = 1 << 1;
    pub const Z: i32 = 1 << 2;
    pub const YAW: i32 = 1 << 3;
    pub const PITCH: i32 = 1 << 4;
    pub const DELTA_X: i32 = 1 << 5;
    pub const DELTA_Y: i32 = 1 << 6;
    pub const DELTA_Z: i32 = 1 << 7;
    pub const ROTATE_DELTA: i32 = 1 << 8;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerPosition {
    pub id: i32,
    pub pos: [f64; 3],
    pub delta: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub relative: i32,
}

pub fn read_player_position(r: &mut Reader<'_>) -> Result<PlayerPosition> {
    let id = read_varint(r)?;
    let pos = [f64::decode(r)?, f64::decode(r)?, f64::decode(r)?];
    let delta = [f64::decode(r)?, f64::decode(r)?, f64::decode(r)?];
    Ok(PlayerPosition { id, pos, delta, yaw: f32::decode(r)?, pitch: f32::decode(r)?, relative: i32::decode(r)? })
}

/// Packed chunk position: x in the low bits, z in the high bits.
pub fn read_chunk_pos(r: &mut Reader<'_>) -> Result<(i32, i32)> {
    let v = i64::decode(r)?;
    Ok((v as i32, (v >> 32) as i32))
}

/// Updated clock: identifier, total ticks, tick fraction, speed.
pub type ClockUpdate = (i32, i64, f32, f32);

/// Time packet: game time, then updated clocks (identifier in the `world_clock` registry,
/// total ticks, tick fraction, speed; 0 = paused).
pub fn read_set_time(r: &mut Reader<'_>) -> Result<(i64, Vec<ClockUpdate>)> {
    let game_time = i64::decode(r)?;
    // Minimal entry: identifier, ticks, two floats.
    let n = rc_proto::read_count(r, 10)?;
    let mut clocks = Vec::with_capacity(n);
    for _ in 0..n {
        clocks.push((read_varint(r)?, read_varlong(r)?, f32::decode(r)?, f32::decode(r)?));
    }
    Ok((game_time, clocks))
}

/// World spawn point: dimension, block position, yaw, pitch.
pub fn read_spawn_position(r: &mut Reader<'_>) -> Result<(String, BlockPos, f32, f32)> {
    let dim = rc_proto::read_str(r, 32767)?.to_owned();
    Ok((dim, BlockPos::decode(r)?, f32::decode(r)?, f32::decode(r)?))
}

/// Local player health: health, food, saturation.
pub fn read_set_health(r: &mut Reader<'_>) -> Result<(f32, i32, f32)> {
    Ok((f32::decode(r)?, read_varint(r)?, f32::decode(r)?))
}

/// Experience: progress within the level (0..1), level, total.
pub fn read_set_experience(r: &mut Reader<'_>) -> Result<(f32, i32, i32)> {
    Ok((f32::decode(r)?, read_varint(r)?, read_varint(r)?))
}

/// Game event: type (byte) and parameter.
pub fn read_game_event(r: &mut Reader<'_>) -> Result<(u8, f32)> {
    Ok((u8::decode(r)?, f32::decode(r)?))
}

pub fn read_block_update(r: &mut Reader<'_>) -> Result<(BlockPos, u32)> {
    Ok((BlockPos::decode(r)?, read_varint(r)? as u32))
}

/// `block_event`: position, type (`b0`), parameter (`b1`), block (`block-entities.md` § 1.4).
pub fn read_block_event(r: &mut Reader<'_>) -> Result<(BlockPos, u8, u8, i32)> {
    Ok((BlockPos::decode(r)?, u8::decode(r)?, u8::decode(r)?, read_varint(r)?))
}

/// `block_entity_data`: position, type (registry `block_entity_type`), NBT (§ 1.2).
pub fn read_block_entity_data(r: &mut Reader<'_>) -> Result<(BlockPos, i32, Option<rc_proto::nbt::Nbt>)> {
    Ok((BlockPos::decode(r)?, read_varint(r)?, rc_proto::nbt::read(r)?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionUpdate {
    pub section: [i32; 3],
    /// (local index `y·256 + z·16 + x`, state).
    pub changes: Vec<(u16, u32)>,
}

pub fn read_section_update(r: &mut Reader<'_>) -> Result<SectionUpdate> {
    let v = i64::decode(r)?;
    let section = [(v >> 42) as i32, ((v << 44) >> 44) as i32, ((v << 22) >> 42) as i32];
    let n = rc_proto::read_count(r, 1)?;
    let mut changes = Vec::with_capacity(n);
    for _ in 0..n {
        let c = read_varlong(r)?;
        let local = (c & 0xfff) as u16;
        // The network stores x, z, y (4 bits each); we convert to the section index.
        let (x, z, y) = (local >> 8 & 15, local >> 4 & 15, local & 15);
        changes.push(((y << 8) | (z << 4) | x, (c as u64 >> 12) as u32));
    }
    Ok(SectionUpdate { section, changes })
}

// --- Packets sent: body without identifier (added by the caller). ---

pub fn write_handshake(w: &mut Vec<u8>, protocol: i32, host: &str, port: u16, intent: i32) {
    write_varint(w, protocol);
    write_str(w, host);
    port.encode(w);
    write_varint(w, intent);
}

pub fn write_hello(w: &mut Vec<u8>, name: &str, uuid: u128) {
    write_str(w, name);
    uuid.encode(w);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation {
    pub language: String,
    pub view_distance: u8,
    pub chat_visibility: i32,
    pub chat_colors: bool,
    pub skin_parts: u8,
    pub main_hand: i32,
    pub text_filtering: bool,
    pub allow_listing: bool,
    pub particles: i32,
}

impl Default for ClientInformation {
    fn default() -> Self {
        ClientInformation {
            language: "fr_fr".into(),
            view_distance: 12,
            chat_visibility: 0,
            chat_colors: true,
            skin_parts: 0x7f,
            main_hand: 1,
            text_filtering: false,
            allow_listing: true,
            particles: 0,
        }
    }
}

pub fn write_client_information(w: &mut Vec<u8>, c: &ClientInformation) {
    write_str(w, &c.language);
    w.push(c.view_distance);
    write_varint(w, c.chat_visibility);
    c.chat_colors.encode(w);
    w.push(c.skin_parts);
    write_varint(w, c.main_hand);
    c.text_filtering.encode(w);
    c.allow_listing.encode(w);
    write_varint(w, c.particles);
}

pub fn write_accept_teleportation(w: &mut Vec<u8>, id: i32, pos: [f64; 3], yaw: f32, pitch: f32) {
    write_varint(w, id);
    for v in pos {
        v.encode(w);
    }
    yaw.encode(w);
    pitch.encode(w);
}

/// Official computation of the chunk send rate wanted by the client.
#[derive(Debug, Clone)]
pub struct ChunkBatchRate {
    nanos_per_chunk: f64,
    weight: u32,
}

impl Default for ChunkBatchRate {
    fn default() -> Self {
        ChunkBatchRate { nanos_per_chunk: 2_000_000.0, weight: 1 }
    }
}

impl ChunkBatchRate {
    pub fn on_batch(&mut self, size: i32, elapsed_nanos: f64) {
        if size > 0 {
            let sample =
                (elapsed_nanos / f64::from(size)).clamp(self.nanos_per_chunk / 3.0, self.nanos_per_chunk * 3.0);
            self.nanos_per_chunk =
                (self.nanos_per_chunk * f64::from(self.weight) + sample) / f64::from(self.weight + 1);
            self.weight = (self.weight + 1).min(49);
        }
    }

    pub fn desired_chunks_per_tick(&self) -> f32 {
        (7_000_000.0 / self.nanos_per_chunk) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_update_packing() {
        let mut w = Vec::new();
        let sec: i64 = ((-3i64 & 0x3f_ffff) << 42) | ((5i64 & 0x3f_ffff) << 20) | (-4i64 & 0xf_ffff);
        sec.encode(&mut w);
        write_varint(&mut w, 1);
        // state 42 at x=1, z=2, y=3.
        rc_proto::varint::write_varlong(&mut w, (42 << 12) | (1 << 8) | (2 << 4) | 3);
        let u = read_section_update(&mut Reader::new(&w)).unwrap();
        assert_eq!(u.section, [-3, -4, 5]);
        assert_eq!(u.changes, [((3 << 8) | (2 << 4) | 1, 42)]);
    }

    #[test]
    fn batch_rate_starts_like_official_and_caps_by_measure() {
        let mut r = ChunkBatchRate::default();
        assert_eq!(r.desired_chunks_per_tick(), 3.5);
        // Batches processed very fast (6 µs per chunk): the official weighted average descends
        // gradually; the server ceiling (64) is exceeded at the 80th batch.
        let mut last = r.desired_chunks_per_tick();
        for i in 1..=80 {
            r.on_batch(10, 60_000.0);
            let now = r.desired_chunks_per_tick();
            assert!(now > last);
            assert_eq!(now > 64.0, i == 80, "batch {i}: {now}");
            last = now;
        }
    }
}

// --- Abilities, attributes, tags ---

/// Player abilities (game mode).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Abilities {
    pub invulnerable: bool,
    pub flying: bool,
    pub may_fly: bool,
    pub instabuild: bool,
    pub flying_speed: f32,
    pub walking_speed: f32,
}

pub fn read_abilities(r: &mut Reader<'_>) -> Result<Abilities> {
    let f = r.u8()?;
    Ok(Abilities {
        invulnerable: f & 1 != 0,
        flying: f & 2 != 0,
        may_fly: f & 4 != 0,
        instabuild: f & 8 != 0,
        flying_speed: f32::decode(r)?,
        walking_speed: f32::decode(r)?,
    })
}

/// Flags sent by the client: only "flying" (bit 2) is transmitted.
pub fn write_abilities(w: &mut Vec<u8>, flying: bool) {
    w.push(if flying { 2 } else { 0 });
}

/// Attribute modifier: operation 0 add, 1 multiply base, 2 multiply total.
#[derive(Debug, Clone, PartialEq)]
pub struct Modifier {
    pub id: String,
    pub amount: f64,
    pub operation: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    /// Network identifier in the attribute registry.
    pub id: i32,
    pub base: f64,
    pub modifiers: Vec<Modifier>,
}

pub fn read_update_attributes(r: &mut Reader<'_>) -> Result<(i32, Vec<Attribute>)> {
    let entity = read_varint(r)?;
    let n = read_varint(r)?;
    if !(0..=128).contains(&n) {
        return Err(rc_proto::Error::LengthOutOfRange { len: n.into(), max: 128 });
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let id = read_varint(r)?;
        let base = f64::decode(r)?;
        // Minimal modifier: empty identifier, f64, operation.
        let m = rc_proto::read_count(r, 10)?;
        let modifiers = (0..m)
            .map(|_| {
                Ok(Modifier {
                    id: rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT)?.to_owned(),
                    amount: f64::decode(r)?,
                    operation: read_varint(r)?,
                })
            })
            .collect::<Result<_>>()?;
        out.push(Attribute { id, base, modifiers });
    }
    Ok((entity, out))
}

/// Tags per registry: registry → (tag → network identifiers).
pub type Tags = Vec<(String, Vec<(String, Vec<i32>)>)>;

pub fn read_update_tags(r: &mut Reader<'_>) -> Result<Tags> {
    use rc_proto::read_count;
    // Minimal registry and tag: empty name + zero count (2 bytes); identifier: 1 byte.
    let n = read_count(r, 2)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let registry = rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT)?.to_owned();
        let t = read_count(r, 2)?;
        let mut tags = Vec::with_capacity(t);
        for _ in 0..t {
            let name = rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT)?.to_owned();
            let k = read_count(r, 1)?;
            tags.push((name, (0..k).map(|_| read_varint(r)).collect::<Result<_>>()?));
        }
        out.push((registry, tags));
    }
    Ok(out)
}

/// Decoration of a chat type: translation key, parameters (0 sender, 1 target,
/// 2 content), style (NBT of a component).
#[derive(Debug, Clone, PartialEq)]
pub struct Decoration {
    pub key: String,
    pub params: Vec<u8>,
    pub style: Option<rc_proto::nbt::Nbt>,
}

/// Chat type reference: index in the `chat_type` registry, or inline type.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatTypeRef {
    Registry(u32),
    Inline(Decoration),
}

fn read_decoration(r: &mut Reader<'_>) -> Result<Decoration> {
    let key = rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT)?.to_owned();
    let n = rc_proto::read_count(r, 1)?;
    let mut params = Vec::with_capacity(n);
    for _ in 0..n {
        let p = read_varint(r)?;
        params.push(if (0..=2).contains(&p) { p as u8 } else { 0 });
    }
    Ok(Decoration { key, params, style: rc_proto::nbt::read(r)? })
}

fn read_chat_type(r: &mut Reader<'_>) -> Result<ChatTypeRef> {
    let v = read_varint(r)?;
    if v > 0 {
        return Ok(ChatTypeRef::Registry(v as u32 - 1));
    }
    let chat = read_decoration(r)?;
    let _narration = read_decoration(r)?;
    Ok(ChatTypeRef::Inline(chat))
}

fn read_opt_nbt(r: &mut Reader<'_>) -> Result<Option<rc_proto::nbt::Nbt>> {
    if bool::decode(r)? { rc_proto::nbt::read(r) } else { Ok(None) }
}

/// Packed signature: index in the cache (`Err`), or full 256 bytes.
fn read_packed_signature(r: &mut Reader<'_>) -> Result<std::result::Result<[u8; 256], u32>> {
    let v = read_varint(r)?;
    if v == 0 { Ok(Ok(r.take_array()?)) } else { Ok(Err(v as u32 - 1)) }
}

/// Player message (`docs/specs/chat.md` § 1.2).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerChat {
    pub global_index: i32,
    pub sender: u128,
    /// Index of the message in the sender's chat session.
    pub session_index: i32,
    pub signature: Option<Box<[u8; 256]>>,
    pub content: String,
    /// Timestamp (ms since the epoch) and salt of the signed body.
    pub timestamp_ms: i64,
    pub salt: i64,
    pub last_seen: Vec<std::result::Result<[u8; 256], u32>>,
    pub unsigned: Option<rc_proto::nbt::Nbt>,
    /// Mask: 0 none, 1 all filtered, 2 partial (bits per character).
    pub filter: (u8, Vec<u8>),
    pub chat_type: ChatTypeRef,
    pub sender_name: Option<rc_proto::nbt::Nbt>,
    pub target_name: Option<rc_proto::nbt::Nbt>,
}

pub fn read_player_chat(r: &mut Reader<'_>) -> Result<PlayerChat> {
    let global_index = read_varint(r)?;
    let sender = u128::from(u64::decode(r)?) << 64 | u128::from(u64::decode(r)?);
    let session_index = read_varint(r)?;
    let signature = if bool::decode(r)? { Some(Box::new(r.take_array()?)) } else { None };
    let content = rc_proto::read_str(r, 256)?.to_owned();
    let timestamp_ms = i64::decode(r)?;
    let salt = i64::decode(r)?;
    let n = rc_proto::read_count(r, 1)?;
    if n > 20 {
        return Err(rc_proto::Error::LengthOutOfRange { len: n as i64, max: 20 });
    }
    let last_seen = (0..n).map(|_| read_packed_signature(r)).collect::<Result<_>>()?;
    let unsigned = read_opt_nbt(r)?;
    let kind = read_varint(r)?;
    let filter = match kind {
        2 => {
            let len = rc_proto::read_count(r, 1)?;
            (2, r.take(len)?.to_vec())
        }
        1 => (1, Vec::new()),
        _ => (0, Vec::new()),
    };
    let chat_type = read_chat_type(r)?;
    let sender_name = rc_proto::nbt::read(r)?;
    let target_name = read_opt_nbt(r)?;
    Ok(PlayerChat {
        global_index,
        sender,
        session_index,
        signature,
        content,
        timestamp_ms,
        salt,
        last_seen,
        unsigned,
        filter,
        chat_type,
        sender_name,
        target_name,
    })
}

/// Sound event (`sounds.md` § 1.1): identifier from the frozen registry, or inline-defined name.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundRef {
    Id(i32),
    Named(String),
}

pub fn read_sound_ref(r: &mut Reader<'_>) -> Result<SoundRef> {
    let k = read_varint(r)?;
    if k > 0 {
        return Ok(SoundRef::Id(k - 1));
    }
    let name = rc_proto::read_str(r, 32767)?.to_owned();
    if bool::decode(r)? {
        let _range = f32::decode(r)?;
    }
    Ok(SoundRef::Named(name))
}

/// `sound` (position at 1/8 block) or `sound_entity` (entity).
#[derive(Debug, Clone, PartialEq)]
pub struct SoundPacket {
    pub sound: SoundRef,
    pub category: i32,
    pub pos: Option<[f64; 3]>,
    pub entity: Option<i32>,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

pub fn read_sound(r: &mut Reader<'_>) -> Result<SoundPacket> {
    let sound = read_sound_ref(r)?;
    let category = read_varint(r)?;
    let pos = [i32::decode(r)?, i32::decode(r)?, i32::decode(r)?].map(|v| f64::from(v) / 8.0);
    Ok(SoundPacket {
        sound,
        category,
        pos: Some(pos),
        entity: None,
        volume: f32::decode(r)?,
        pitch: f32::decode(r)?,
        seed: i64::decode(r)?,
    })
}

pub fn read_sound_entity(r: &mut Reader<'_>) -> Result<SoundPacket> {
    let sound = read_sound_ref(r)?;
    let category = read_varint(r)?;
    let entity = read_varint(r)?;
    Ok(SoundPacket {
        sound,
        category,
        pos: None,
        entity: Some(entity),
        volume: f32::decode(r)?,
        pitch: f32::decode(r)?,
        seed: i64::decode(r)?,
    })
}

/// `level_event`: type, block position, data, global.
pub fn read_level_event(r: &mut Reader<'_>) -> Result<(i32, BlockPos, i32, bool)> {
    Ok((i32::decode(r)?, BlockPos::decode(r)?, i32::decode(r)?, bool::decode(r)?))
}

/// `stop_sound`: optional category and event.
pub fn read_stop_sound(r: &mut Reader<'_>) -> Result<(Option<i32>, Option<String>)> {
    let flags = r.u8()?;
    let category = if flags & 1 != 0 { Some(read_varint(r)?) } else { None };
    let name = if flags & 2 != 0 { Some(rc_proto::read_str(r, 32767)?.to_owned()) } else { None };
    Ok((category, name))
}

/// Operation of `boss_event` (`hud-advanced.md` § 1.1).
#[derive(Debug, Clone, PartialEq)]
pub enum BossOp {
    Add { name: Option<rc_proto::nbt::Nbt>, progress: f32, color: i32, style: i32, flags: u8 },
    Remove,
    Progress(f32),
    Name(Option<rc_proto::nbt::Nbt>),
    Style { color: i32, style: i32 },
    Flags(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct BossEvent {
    pub id: u128,
    pub op: BossOp,
}

pub fn read_boss_event(r: &mut Reader<'_>) -> Result<BossEvent> {
    let id = u128::decode(r)?;
    let op = match read_varint(r)? {
        0 => BossOp::Add {
            name: rc_proto::nbt::read(r)?,
            progress: f32::decode(r)?,
            color: read_varint(r)?,
            style: read_varint(r)?,
            flags: r.u8()?,
        },
        1 => BossOp::Remove,
        2 => BossOp::Progress(f32::decode(r)?),
        3 => BossOp::Name(rc_proto::nbt::read(r)?),
        4 => BossOp::Style { color: read_varint(r)?, style: read_varint(r)? },
        5 => BossOp::Flags(r.u8()?),
        op => return Err(rc_proto::Error::InvalidValue { what: "boss bar operation", value: i64::from(op) }),
    };
    Ok(BossEvent { id, op })
}

/// Title durations: fade-in, stay, fade-out (`i32`, ticks).
pub fn read_titles_animation(r: &mut Reader<'_>) -> Result<[i32; 3]> {
    Ok([i32::decode(r)?, i32::decode(r)?, i32::decode(r)?])
}

/// Header and footer of the player list (components; empty text = none).
pub fn read_tab_list(r: &mut Reader<'_>) -> Result<(Option<rc_proto::nbt::Nbt>, Option<rc_proto::nbt::Nbt>)> {
    Ok((rc_proto::nbt::read(r)?, rc_proto::nbt::read(r)?))
}

/// System message: component and "overlay" (action bar).
pub fn read_system_chat(r: &mut Reader<'_>) -> Result<(Option<rc_proto::nbt::Nbt>, bool)> {
    Ok((rc_proto::nbt::read(r)?, bool::decode(r)?))
}

/// Disguised message: message, chat type, sender, target.
pub type DisguisedChat =
    (Option<rc_proto::nbt::Nbt>, ChatTypeRef, Option<rc_proto::nbt::Nbt>, Option<rc_proto::nbt::Nbt>);

pub fn read_disguised_chat(r: &mut Reader<'_>) -> Result<DisguisedChat> {
    Ok((rc_proto::nbt::read(r)?, read_chat_type(r)?, rc_proto::nbt::read(r)?, read_opt_nbt(r)?))
}

/// Body of the `chat` packet without a chat session: no signature, last seen empty
/// (offset 0, 3 null bytes, checksum 1).
pub fn write_chat(w: &mut Vec<u8>, message: &str, timestamp_ms: i64, salt: i64) {
    write_str(w, message);
    timestamp_ms.encode(w);
    salt.encode(w);
    false.encode(w);
    write_varint(w, 0);
    w.extend_from_slice(&[0, 0, 0, 1]);
}
