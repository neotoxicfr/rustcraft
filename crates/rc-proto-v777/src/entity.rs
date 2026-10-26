//! Entity packets and metadata (`docs/specs/entities.md` part 1).

use rc_proto::varint::{read_varint, read_varlong};
use rc_proto::{BlockPos, Decode, Error, Reader, Result, read_str};

use crate::item::{ItemCodec, ItemStack};

fn bad(what: &'static str, value: i64) -> Error {
    Error::InvalidValue { what, value }
}

fn vec3d(r: &mut Reader<'_>) -> Result<[f64; 3]> {
    Ok([f64::decode(r)?, f64::decode(r)?, f64::decode(r)?])
}

/// One-byte angle → degrees.
fn angle(r: &mut Reader<'_>) -> Result<f32> {
    Ok(f32::from(i8::decode(r)?) * 360.0 / 256.0)
}

/// "Low precision" velocity (blocks per tick): three 15-bit components and a scale.
pub fn read_lp_vec3(r: &mut Reader<'_>) -> Result<[f64; 3]> {
    let o0 = u8::decode(r)?;
    if o0 == 0 {
        return Ok([0.0; 3]);
    }
    let o1 = u8::decode(r)?;
    let o2 = u32::decode(r)?;
    let b = u64::from(o2) << 16 | u64::from(o1) << 8 | u64::from(o0);
    let mut scale = u64::from(o0 & 3);
    if o0 & 4 != 0 {
        scale |= u64::from(read_varint(r)? as u32) << 2;
    }
    let comp = |shift: u32| {
        let q = ((b >> shift) & 0x7fff).min(32766) as f64;
        (q * 2.0 / 32766.0 - 1.0) * scale as f64
    };
    Ok([comp(3), comp(18), comp(33)])
}

/// Position path: steps (absolute position, duration in ticks); a linear path has a
/// single step of duration 0.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionPath {
    pub linear: bool,
    pub steps: Vec<([f64; 3], i32)>,
}

impl PositionPath {
    pub fn end(&self) -> [f64; 3] {
        self.steps.last().map_or([0.0; 3], |s| s.0)
    }
}

/// Delta relative to a base: linear or stepwise (units of 1/4096 block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Linear([i16; 3]),
    Steps(Vec<(i32, [i16; 3])>),
}

/// Coordinate in delta units (1/4096 block), rounded like the game: to nearest, halves
/// up (−2.5 → −2), and not away from zero like `f64::round`.
pub fn delta_unit(v: f64) -> i64 {
    let x = v * 4096.0;
    let f = x.floor();
    (if x - f >= 0.5 { f + 1.0 } else { f }) as i64
}

/// Decoded position of a delta (§ 1.0.2): an axis at 0 keeps exactly the base.
pub fn apply_delta(base: [f64; 3], d: [i16; 3]) -> [f64; 3] {
    std::array::from_fn(|i| if d[i] == 0 { base[i] } else { (delta_unit(base[i]) + i64::from(d[i])) as f64 / 4096.0 })
}

impl Delta {
    pub fn path(&self, base: [f64; 3]) -> PositionPath {
        match self {
            Delta::Linear(d) => PositionPath { linear: true, steps: vec![(apply_delta(base, *d), 0)] },
            Delta::Steps(list) => {
                let mut at = base;
                let steps = list
                    .iter()
                    .map(|(t, d)| {
                        at = apply_delta(at, *d);
                        (at, *t)
                    })
                    .collect();
                PositionPath { linear: false, steps }
            }
        }
    }
}

fn read_delta(r: &mut Reader<'_>, props: i32) -> Result<Delta> {
    let d = |r: &mut Reader<'_>| -> Result<[i16; 3]> { Ok([i16::decode(r)?, i16::decode(r)?, i16::decode(r)?]) };
    let n = (props as u32 >> 1) as usize;
    if n == 0 {
        return Ok(Delta::Linear(d(r)?));
    }
    if n > r.remaining() / 7 {
        return Err(bad("movement steps", n as i64));
    }
    (0..n).map(|_| Ok((read_varint(r)?, d(r)?))).collect::<Result<_>>().map(Delta::Steps)
}

fn read_path(r: &mut Reader<'_>) -> Result<PositionPath> {
    if read_varint(r)? == 1 {
        let n = rc_proto::read_count(r, 25)?;
        let steps = (0..n).map(|_| Ok((vec3d(r)?, read_varint(r)?))).collect::<Result<_>>()?;
        Ok(PositionPath { linear: false, steps })
    } else {
        Ok(PositionPath { linear: true, steps: vec![(vec3d(r)?, 0)] })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddEntity {
    pub id: i32,
    pub uuid: u128,
    pub kind: i32,
    pub pos: [f64; 3],
    pub velocity: [f64; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub head_yaw: f32,
    pub data: i32,
}

pub fn read_add_entity(r: &mut Reader<'_>) -> Result<AddEntity> {
    let id = read_varint(r)?;
    let uuid = u128::from_be_bytes(r.take_array()?);
    let kind = read_varint(r)?;
    let pos = vec3d(r)?;
    let velocity = read_lp_vec3(r)?;
    Ok(AddEntity {
        id,
        uuid,
        kind,
        pos,
        velocity,
        pitch: angle(r)?,
        yaw: angle(r)?,
        head_yaw: angle(r)?,
        data: read_varint(r)?,
    })
}

pub fn read_remove_entities(r: &mut Reader<'_>) -> Result<Vec<i32>> {
    let n = rc_proto::read_count(r, 1)?;
    (0..n).map(|_| read_varint(r)).collect()
}

/// Relative movement: identifier, on ground, delta, optional rotation (yaw, pitch).
#[derive(Debug, Clone, PartialEq)]
pub struct Move {
    pub id: i32,
    pub on_ground: bool,
    pub delta: Option<Delta>,
    pub rot: Option<(f32, f32)>,
}

pub fn read_move_pos(r: &mut Reader<'_>, rot: bool) -> Result<Move> {
    let id = read_varint(r)?;
    let props = read_varint(r)?;
    let delta = read_delta(r, props)?;
    let rot = if rot { Some((angle(r)?, angle(r)?)) } else { None };
    Ok(Move { id, on_ground: props & 1 != 0, delta: Some(delta), rot })
}

pub fn read_move_rot(r: &mut Reader<'_>) -> Result<Move> {
    let id = read_varint(r)?;
    let on_ground = bool::decode(r)?;
    let rot = Some((angle(r)?, angle(r)?));
    Ok(Move { id, on_ground, delta: None, rot })
}

#[derive(Debug, Clone, PartialEq)]
pub struct TeleportEntity {
    pub id: i32,
    pub pos: [f64; 3],
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub relative: i32,
    pub on_ground: bool,
}

pub fn read_teleport_entity(r: &mut Reader<'_>) -> Result<TeleportEntity> {
    Ok(TeleportEntity {
        id: read_varint(r)?,
        pos: vec3d(r)?,
        velocity: vec3d(r)?,
        yaw: f32::decode(r)?,
        pitch: f32::decode(r)?,
        relative: i32::decode(r)?,
        on_ground: bool::decode(r)?,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct PositionSync {
    pub id: i32,
    pub path: PositionPath,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

pub fn read_position_sync(r: &mut Reader<'_>) -> Result<PositionSync> {
    Ok(PositionSync {
        id: read_varint(r)?,
        path: read_path(r)?,
        yaw: f32::decode(r)?,
        pitch: f32::decode(r)?,
        on_ground: bool::decode(r)?,
    })
}

pub fn read_rotate_head(r: &mut Reader<'_>) -> Result<(i32, f32)> {
    Ok((read_varint(r)?, angle(r)?))
}

pub fn read_motion(r: &mut Reader<'_>) -> Result<(i32, [f64; 3])> {
    Ok((read_varint(r)?, read_lp_vec3(r)?))
}

/// Metadata value (§ 2.2).
#[derive(Debug, Clone, PartialEq)]
pub enum Meta {
    Byte(i8),
    Int(i32),
    Long(i64),
    Float(f32),
    String(String),
    /// Text component (raw NBT), absent for an empty optional component.
    Text(Option<rc_proto::nbt::Nbt>),
    Item(Option<ItemStack>),
    Bool(bool),
    Rotations([f32; 3]),
    BlockPos(Option<[i32; 3]>),
    Direction(i32),
    Entity(Option<u128>),
    BlockState(Option<i32>),
    Particles(Vec<Particle>),
    /// Particle alone (serializer 16: area effect cloud).
    Particle(Particle),
    /// Villager type, profession, level.
    Villager([i32; 3]),
    OptionalInt(Option<i32>),
    Pose(i32),
    /// Variant or state coded by an identifier (cat, cow, wolf… animal states, arm,
    /// dye), with the serializer identifier (§ 2.2).
    Variant(i32, u8),
    GlobalPos(Option<(String, [i32; 3])>),
    Vector3([f32; 3]),
    Quaternion([f32; 4]),
    /// Resolvable profile (raw bytes, encoding of the `profile` component).
    Profile(Box<[u8]>),
}

/// Particle: type and raw options.
#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    pub kind: i32,
    /// ARGB colour for tinted particles (`entity_effect`…), RGB for `dust`, otherwise 0.
    pub color: u32,
    /// Block state (`block`…) or integer (`shriek`), otherwise 0.
    pub state: i32,
    /// Item (`item`), otherwise −1.
    pub item: i32,
    /// Scale (`dust`) or power (`effect`), otherwise 1.
    pub scale: f32,
    /// End RGB colour of `dust_color_transition` (`color` = start), otherwise 0.
    pub color2: u32,
    /// `vibration`: destination and trip duration in ticks, otherwise `None`.
    pub vib: Option<(VibDest, i32)>,
}

/// Destination of a vibration particle (the game's position source).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VibDest {
    /// Centre of a block.
    Block([i32; 3]),
    /// Tracked entity, at this height above its feet.
    Entity { id: i32, y_offset: f32 },
}

/// `level_particles` (`particles.md` § 1.1).
#[derive(Debug, Clone, PartialEq)]
pub struct LevelParticles {
    pub particle: Particle,
    pub force: bool,
    pub always_visible: bool,
    pub pos: [f64; 3],
    pub delta: [f32; 3],
    pub speed: [f32; 3],
    pub count: i32,
    /// 0 default (Gaussian), 1 alternative, 2 alternative with speed.
    pub mode: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParticleOpts {
    None,
    State,
    Dust,
    DustTransition,
    Color,
    ColorPower,
    Float,
    VarInt,
    Item,
    Vibration,
    Trail,
    Geyser,
    GeyserBase,
}

fn block_pos(r: &mut Reader<'_>) -> Result<[i32; 3]> {
    let p = BlockPos::decode(r)?;
    Ok([p.x, p.y, p.z])
}

/// Equipment received: (slot, stack).
pub type Equipment = Vec<(u8, Option<ItemStack>)>;

/// Metadata decoder (item stacks and particles according to the version's registries).
#[derive(Debug, Clone, Default)]
pub struct MetaCodec {
    items: ItemCodec,
    particles: Vec<ParticleOpts>,
}

impl MetaCodec {
    /// `particle_types`: names of the `minecraft:particle_type` registry in identifier order.
    pub fn new(items: ItemCodec, particle_types: &[String]) -> Self {
        let particles = particle_types
            .iter()
            .map(|n| match n.strip_prefix("minecraft:").unwrap_or(n) {
                "block" | "block_marker" | "falling_dust" | "dust_pillar" | "block_crumble" => ParticleOpts::State,
                "dust" => ParticleOpts::Dust,
                "dust_color_transition" => ParticleOpts::DustTransition,
                "entity_effect" | "tinted_leaves" | "flash" => ParticleOpts::Color,
                "effect" | "instant_effect" => ParticleOpts::ColorPower,
                "dragon_breath" | "sculk_charge" => ParticleOpts::Float,
                "shriek" => ParticleOpts::VarInt,
                "item" => ParticleOpts::Item,
                "vibration" => ParticleOpts::Vibration,
                "trail" => ParticleOpts::Trail,
                "geyser" | "geyser_plume" => ParticleOpts::Geyser,
                "geyser_base" | "geyser_poof" => ParticleOpts::GeyserBase,
                _ => ParticleOpts::None,
            })
            .collect();
        MetaCodec { items, particles }
    }

    pub fn items(&self) -> &ItemCodec {
        &self.items
    }

    /// `level_particles`: particle, flags, centre, spread, speed, count, random draw.
    pub fn read_level_particles(&self, r: &mut Reader<'_>) -> Result<LevelParticles> {
        let particle = self.particle(r)?;
        let force = bool::decode(r)?;
        let always_visible = bool::decode(r)?;
        let pos = [f64::decode(r)?, f64::decode(r)?, f64::decode(r)?];
        let delta = [f32::decode(r)?, f32::decode(r)?, f32::decode(r)?];
        let speed = [f32::decode(r)?, f32::decode(r)?, f32::decode(r)?];
        let count = read_varint(r)?;
        let mode = read_varint(r)?;
        Ok(LevelParticles {
            particle,
            force,
            always_visible,
            pos,
            delta,
            speed,
            count,
            mode: if (0..3).contains(&mode) { mode } else { 0 },
        })
    }

    pub fn particle(&self, r: &mut Reader<'_>) -> Result<Particle> {
        let kind = read_varint(r)?;
        let opts = usize::try_from(kind).ok().and_then(|k| self.particles.get(k)).copied();
        let (mut color, mut state, mut item, mut scale, mut color2) = (0, 0, -1, 1.0, 0);
        let mut vib = None;
        match opts {
            None => return Err(bad("particle type", i64::from(kind))),
            Some(ParticleOpts::None) => {}
            Some(ParticleOpts::State | ParticleOpts::VarInt) => {
                state = read_varint(r)?;
            }
            Some(ParticleOpts::Dust) => {
                color = u32::decode(r)?;
                scale = f32::decode(r)?;
            }
            Some(ParticleOpts::DustTransition) => {
                color = u32::decode(r)?;
                color2 = u32::decode(r)?;
                scale = f32::decode(r)?;
            }
            Some(ParticleOpts::Color) => color = u32::decode(r)?,
            Some(ParticleOpts::ColorPower) => {
                color = u32::decode(r)?;
                scale = f32::decode(r)?;
            }
            // Power (`dragon_breath`) or roll (`sculk_charge`), read like the scale.
            Some(ParticleOpts::Float) => scale = f32::decode(r)?,
            Some(ParticleOpts::Geyser) => {
                r.take(4)?;
            }
            Some(ParticleOpts::GeyserBase) => {
                r.take(8)?;
            }
            Some(ParticleOpts::Item) => {
                item = self.items.read_template(r)?.item;
            }
            Some(ParticleOpts::Vibration) => {
                let dest = match read_varint(r)? {
                    0 => VibDest::Block(block_pos(r)?),
                    1 => {
                        let id = read_varint(r)?;
                        VibDest::Entity { id, y_offset: f32::decode(r)? }
                    }
                    k => return Err(bad("vibration source", i64::from(k))),
                };
                vib = Some((dest, read_varint(r)?));
            }
            Some(ParticleOpts::Trail) => {
                vec3d(r)?;
                color = u32::decode(r)?;
                read_varint(r)?;
            }
        }
        Ok(Particle { kind, color, state, item, scale, color2, vib })
    }

    fn value(&self, r: &mut Reader<'_>, ty: i32) -> Result<Meta> {
        Ok(match ty {
            0 => Meta::Byte(i8::decode(r)?),
            1 => Meta::Int(read_varint(r)?),
            2 => Meta::Long(read_varlong(r)?),
            3 => Meta::Float(f32::decode(r)?),
            4 => Meta::String(read_str(r, 32767)?.to_owned()),
            5 => Meta::Text(rc_proto::nbt::read(r)?),
            6 => Meta::Text(if bool::decode(r)? { rc_proto::nbt::read(r)? } else { None }),
            7 => Meta::Item(self.items.read_stack(r)?),
            8 => Meta::Bool(bool::decode(r)?),
            9 => Meta::Rotations([f32::decode(r)?, f32::decode(r)?, f32::decode(r)?]),
            10 => Meta::BlockPos(Some(block_pos(r)?)),
            11 => Meta::BlockPos(if bool::decode(r)? { Some(block_pos(r)?) } else { None }),
            12 => Meta::Direction(read_varint(r)?),
            13 => Meta::Entity(if bool::decode(r)? { Some(u128::from_be_bytes(r.take_array()?)) } else { None }),
            14 => Meta::BlockState(Some(read_varint(r)?)),
            15 => Meta::BlockState(match read_varint(r)? {
                0 => None,
                s => Some(s),
            }),
            16 => Meta::Particle(self.particle(r)?),
            17 => {
                let n = rc_proto::read_count(r, 1)?;
                Meta::Particles((0..n).map(|_| self.particle(r)).collect::<Result<_>>()?)
            }
            18 => Meta::Villager([read_varint(r)?, read_varint(r)?, read_varint(r)?]),
            19 => Meta::OptionalInt(match read_varint(r)? {
                0 => None,
                v => Some(v - 1),
            }),
            20 => Meta::Pose(read_varint(r)?),
            21..=32 | 35..=38 | 42 | 43 => Meta::Variant(read_varint(r)?, ty as u8),
            33 => Meta::GlobalPos(if bool::decode(r)? {
                let dim = read_str(r, 32767)?.to_owned();
                Some((dim, block_pos(r)?))
            } else {
                None
            }),
            34 => {
                let v = read_varint(r)?;
                if v == 0 {
                    read_varint(r)?;
                    read_varint(r)?;
                    read_str(r, 32767)?;
                    if bool::decode(r)? {
                        rc_proto::nbt::skip(r)?;
                    }
                    if bool::decode(r)? {
                        rc_proto::nbt::skip(r)?;
                    }
                }
                Meta::Variant(v - 1, 34)
            }
            39 => Meta::Vector3([f32::decode(r)?, f32::decode(r)?, f32::decode(r)?]),
            40 => Meta::Quaternion([f32::decode(r)?, f32::decode(r)?, f32::decode(r)?, f32::decode(r)?]),
            41 => {
                let start = r.rest();
                let before = r.remaining();
                self.items.skip_profile(r)?;
                Meta::Profile(start[..before - r.remaining()].into())
            }
            _ => return Err(bad("metadata serializer", i64::from(ty))),
        })
    }

    /// `set_entity_data`: identifier then (index, value) up to 0xFF.
    pub fn read_entity_data(&self, r: &mut Reader<'_>) -> Result<(i32, Vec<(u8, Meta)>)> {
        let id = read_varint(r)?;
        let mut values = Vec::new();
        loop {
            let index = u8::decode(r)?;
            if index == 0xff {
                break;
            }
            let ty = read_varint(r)?;
            values.push((index, self.value(r, ty)?));
        }
        Ok((id, values))
    }

    /// `set_equipment`: (slot, stack) up to the entry without the 0x80 bit.
    pub fn read_equipment(&self, r: &mut Reader<'_>) -> Result<(i32, Equipment)> {
        let id = read_varint(r)?;
        let mut out = Vec::new();
        loop {
            let slot = u8::decode(r)?;
            out.push((slot & 0x7f, self.items.read_stack(r)?));
            if slot & 0x80 == 0 {
                break;
            }
        }
        Ok((id, out))
    }
}

/// `swing_animation`: entity, hand, kind (0 none, 1 hit, 2 thrust), duration.
pub fn read_swing(r: &mut Reader<'_>) -> Result<(i32, i32, i32, i32)> {
    Ok((read_varint(r)?, read_varint(r)?, read_varint(r)?, read_varint(r)?))
}

/// `entity_event`: 4-byte identifier, event.
pub fn read_entity_event(r: &mut Reader<'_>) -> Result<(i32, i8)> {
    Ok((i32::decode(r)?, i8::decode(r)?))
}

/// `damage_event`: entity hit and damage type (index of the `damage_type` registry); causes
/// and source position read and ignored.
pub fn read_damage_event(r: &mut Reader<'_>) -> Result<(i32, i32)> {
    let id = read_varint(r)?;
    let source = read_varint(r)?;
    read_varint(r)?;
    read_varint(r)?;
    if bool::decode(r)? {
        vec3d(r)?;
    }
    Ok((id, source))
}

/// `hurt_animation`: entity, direction of the hit (degrees).
pub fn read_hurt_animation(r: &mut Reader<'_>) -> Result<(i32, f32)> {
    Ok((read_varint(r)?, f32::decode(r)?))
}

/// `set_passengers`: vehicle, passengers (the first one drives).
pub fn read_passengers(r: &mut Reader<'_>) -> Result<(i32, Vec<i32>)> {
    let v = read_varint(r)?;
    Ok((v, read_remove_entities(r)?))
}

/// `update_mob_effect`: entity, effect, amplifier, duration, flags.
pub fn read_mob_effect(r: &mut Reader<'_>) -> Result<(i32, i32, i32, i32, i8)> {
    Ok((read_varint(r)?, read_varint(r)?, read_varint(r)?, read_varint(r)?, i8::decode(r)?))
}

/// `remove_mob_effect`: entity, effect.
pub fn read_remove_mob_effect(r: &mut Reader<'_>) -> Result<(i32, i32)> {
    Ok((read_varint(r)?, read_varint(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's rounding (halves up): a negative base at half a unit keeps the unit above,
    /// like the official client (`f64::round` gave the unit below).
    #[test]
    fn delta_rounding_like_the_game() {
        assert_eq!(delta_unit(-2.5 / 4096.0), -2);
        assert_eq!(delta_unit(2.5 / 4096.0), 3);
        assert_eq!(delta_unit(-2.4 / 4096.0), -2);
        assert_eq!(delta_unit(-2.6 / 4096.0), -3);
        assert_eq!(delta_unit(0.49999999999999994 / 4096.0), 0);
        let p = apply_delta([-0.5 / 4096.0, 0.0, 0.0], [1, 0, 0]);
        assert_eq!(p[0], 1.0 / 4096.0);
    }

    #[test]
    fn damage_event_keeps_type() {
        // Entity 300 (2-byte varint), type 17 (drowning), causes 0 and 0, with source position.
        let mut b = vec![0xac, 0x02, 17, 0, 0, 1];
        b.extend([0u8; 24]);
        let mut r = Reader::new(&b);
        assert_eq!(read_damage_event(&mut r).unwrap(), (300, 17));
        assert!(r.is_empty());
    }

    #[test]
    fn lp_vector_round_trip_values() {
        assert_eq!(read_lp_vec3(&mut Reader::new(&[0])).unwrap(), [0.0; 3]);
        // Scale 1: components in the middle (16383) → 0 (to within 1/32766).
        let q: u64 = 16383;
        let b = 1u64 | q << 3 | q << 18 | q << 33;
        let bytes = [
            b as u8,
            (b >> 8) as u8,
            ((b >> 16) >> 24) as u8,
            ((b >> 16) >> 16) as u8,
            ((b >> 16) >> 8) as u8,
            (b >> 16) as u8,
        ];
        let v = read_lp_vec3(&mut Reader::new(&bytes)).unwrap();
        assert!(v.iter().all(|c| c.abs() < 1e-4), "{v:?}");
    }

    /// `dust_color_transition`: start colour, end colour, scale (in that order).
    #[test]
    fn dust_transition_keeps_target_color_and_scale() {
        let c = MetaCodec::new(ItemCodec::default(), &["minecraft:dust_color_transition".to_owned()]);
        let mut b = vec![0u8];
        b.extend(0x0011_2233u32.to_be_bytes());
        b.extend(0x00aa_bbccu32.to_be_bytes());
        b.extend(2.5f32.to_be_bytes());
        let mut r = Reader::new(&b);
        let p = c.particle(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((p.color, p.color2, p.scale), (0x0011_2233, 0x00aa_bbcc, 2.5));
    }

    #[test]
    fn delta_keeps_untouched_axes() {
        let base = [10.123_456_789, 64.0, -3.3];
        let p = apply_delta(base, [4096, 0, -2048]);
        assert_eq!(p[1], 64.0);
        assert!((p[0] - 11.123_535_156_25).abs() < 1e-9);
        assert!((p[2] - (-3.800_048_828_125)).abs() < 1e-9);
    }
}
