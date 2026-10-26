//! Isolated packets (`docs/specs/miscellaneous.md`): leashes, pickup, screen effects,
//! projectile power, transient blocks, experimental minecarts, disk alert, server
//! data, combat. Decoded on the network thread, applied by the game (`apps/client/src/misc.rs`).

use rc_proto::varint::read_varint;
use rc_proto::{BlockPos, Decode, Reader};
use rc_text::Component;

use crate::ids::Ids;
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
    /// `server_data`: MOTD and optional PNG icon.
    ServerData { motd: Component, icon: Option<Vec<u8>> },
}

/// Identifiers of the packets of this module.
pub(crate) struct DiversIds {
    link: i32,
    take: i32,
    post: i32,
    cfg_post: i32,
    power: i32,
    transient: i32,
    minecart: i32,
    disk: i32,
    server_data: i32,
    combat_enter: i32,
    combat_end: i32,
}

pub(crate) fn ids(i: &Ids) -> DiversIds {
    DiversIds {
        link: i.set_entity_link,
        take: i.take_item_entity,
        post: i.post_effects,
        cfg_post: i.cfg_post_effects,
        power: i.projectile_power,
        transient: i.add_transient_block,
        minecart: i.move_minecart_along_track,
        disk: i.low_disk_space_warning,
        server_data: i.server_data,
        combat_enter: i.player_combat_enter,
        combat_end: i.player_combat_end,
    }
}

/// Result: `None` if the packet is not from this module, `Some(None)` if it is read and ignored.
type Decoded = Option<Option<Event>>;

/// Configuration: `post_effects` received before the player, ignored without error (§ 5.3, deliberate choice).
pub(crate) fn config(ids: &DiversIds, id: i32, r: &mut Reader<'_>) -> Result<Decoded, Error> {
    if id != ids.cfg_post {
        return Ok(None);
    }
    read_effects(r)?;
    Ok(Some(None))
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

pub(crate) fn play(ids: &DiversIds, id: i32, r: &mut Reader<'_>) -> Result<Decoded, Error> {
    let d = if id == ids.link {
        Misc::Link { held: i32::decode(r)?, holder: i32::decode(r)? }
    } else if id == ids.take {
        Misc::Take { item: read_varint(r)?, collector: read_varint(r)?, amount: read_varint(r)? }
    } else if id == ids.post {
        Misc::PostEffects(read_effects(r)?)
    } else if id == ids.power {
        Misc::ProjectilePower { id: read_varint(r)?, power: f64::decode(r)? }
    } else if id == ids.transient {
        Misc::TransientBlock { pos: BlockPos::decode(r)?, state: read_varint(r)? }
    } else if id == ids.minecart {
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
        Misc::MinecartSteps { id, steps }
    } else if id == ids.disk {
        Misc::LowDiskSpace
    } else if id == ids.server_data {
        let motd = rc_proto::nbt::read(r)?.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| Component::text(""));
        let icon = if bool::decode(r)? {
            let n = rc_proto::read_len(r, ICON_MAX)?;
            Some(r.take(n)?.to_vec())
        } else {
            None
        };
        Misc::ServerData { motd, icon }
    } else if id == ids.combat_enter {
        // The client does nothing with the combat enter and exit events (§ 5.1).
        return Ok(Some(None));
    } else if id == ids.combat_end {
        let _duration = read_varint(r)?;
        return Ok(Some(None));
    } else {
        return Ok(None);
    };
    Ok(Some(Some(Event::Misc(d))))
}
