//! Isolated packets (`docs/specs/miscellaneous.md`): leashes, pickup, screen effects,
//! projectile power, transient blocks, experimental minecarts, disk alert, server
//! data, combat. Decoded on the network thread, applied by the game (`apps/client/src/misc.rs`).

use rc_proto::varint::read_varint;
use rc_proto::{BlockPos, Decode, Reader};
use rc_text::Component;

use crate::{Error, Event};

/// Trajectory step of an experimental minecart (§ 5.6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinecartStep {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub weight: f32,
}

/// Events of this module (a single `Event::Misc` variant on the session side).
#[derive(Debug, Clone, PartialEq)]
pub enum Misc {
    /// `set_entity_link`: held entity, holder (0 = detached).
    Link { held: i32, holder: i32 },
    /// `take_item_entity`: collected entity, collector, quantity.
    Take { item: i32, collector: i32, amount: i32 },
    /// `post_effects` (game): list of enforced screen effects, in order.
    PostEffects(Vec<String>),
    /// `projectile_power`: acceleration power.
    ProjectilePower { id: i32, power: f64 },
    /// `add_transient_block`: block placed by a falling block.
    TransientBlock { pos: BlockPos, state: i32 },
    /// `move_minecart_along_track`: steps of a minecart.
    MinecartSteps { id: i32, steps: Vec<MinecartStep> },
    /// `low_disk_space_warning`.
    LowDiskSpace,
    /// `server_data`: MOTD and optional PNG icon. The MOTD is boxed: a 176-byte component inline
    /// would make every `Misc` (item pickups included) and every `Event` that large.
    ServerData { motd: Box<Component>, icon: Option<Vec<u8>> },
}

/// Play packets of this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MiscPacket {
    Link,
    Take,
    PostEffects,
    ProjectilePower,
    TransientBlock,
    MinecartSteps,
    LowDiskSpace,
    ServerData,
    CombatEnter,
    CombatEnd,
}

/// Configuration: `post_effects` received before the player, ignored without error (§ 5.3, deliberate choice).
pub(crate) fn config_post_effects(r: &mut Reader<'_>) -> Result<(), Error> {
    read_effects(r)?;
    Ok(())
}

fn read_effects(r: &mut Reader<'_>) -> rc_proto::Result<Vec<String>> {
    let n = rc_proto::read_count(r, 1)?;
    (0..n).map(|_| rc_proto::read_str(r, 32767).map(str::to_owned)).collect()
}

fn vec3(r: &mut Reader<'_>) -> rc_proto::Result<[f64; 3]> {
    Ok([f64::decode(r)?, f64::decode(r)?, f64::decode(r)?])
}

fn angle(r: &mut Reader<'_>) -> rc_proto::Result<f32> {
    Ok(f32::from(i8::decode(r)?) * 360.0 / 256.0)
}

/// Server icon: 1 MiB at most (the game bounds the read to that size).
const ICON_MAX: usize = 1 << 20;

/// Event of a packet of this module; `None`: read and ignored.
pub(crate) fn play(packet: MiscPacket, r: &mut Reader<'_>) -> Result<Option<Event>, Error> {
    let d = match packet {
        MiscPacket::Link => Misc::Link { held: i32::decode(r)?, holder: i32::decode(r)? },
        MiscPacket::Take => Misc::Take { item: read_varint(r)?, collector: read_varint(r)?, amount: read_varint(r)? },
        MiscPacket::PostEffects => Misc::PostEffects(read_effects(r)?),
        MiscPacket::ProjectilePower => Misc::ProjectilePower { id: read_varint(r)?, power: f64::decode(r)? },
        MiscPacket::TransientBlock => Misc::TransientBlock { pos: BlockPos::decode(r)?, state: read_varint(r)? },
        MiscPacket::MinecartSteps => read_minecart_steps(r)?,
        MiscPacket::LowDiskSpace => Misc::LowDiskSpace,
        MiscPacket::ServerData => {
            let motd =
                rc_proto::nbt::read(r)?.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| Component::text(""));
            let icon = if bool::decode(r)? {
                let n = rc_proto::read_len(r, ICON_MAX)?;
                Some(r.take(n)?.to_vec())
            } else {
                None
            };
            Misc::ServerData { motd: Box::new(motd), icon }
        }
        // The client does nothing with the combat enter and exit events (§ 5.1).
        MiscPacket::CombatEnter => return Ok(None),
        MiscPacket::CombatEnd => {
            let _duration = read_varint(r)?;
            return Ok(None);
        }
    };
    Ok(Some(Event::Misc(d)))
}

fn read_minecart_steps(r: &mut Reader<'_>) -> rc_proto::Result<Misc> {
    let id = read_varint(r)?;
    let n = rc_proto::read_count(r, 16 + 16 + 2 + 4)?;
    let mut steps = Vec::with_capacity(n);
    for _ in 0..n {
        let pos = vec3(r)?;
        let vel = vec3(r)?;
        let yaw = angle(r)?;
        let pitch = angle(r)?;
        steps.push(MinecartStep { pos, vel, yaw, pitch, weight: f32::decode(r)? });
    }
    Ok(Misc::MinecartSteps { id, steps })
}
