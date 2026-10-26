//! `waypoint` packet (waypoints of the locator bar, `docs/specs/hud.md` § 4.8).

use rc_proto::varint::read_varint;
use rc_proto::{Decode, Reader, Result};

/// Waypoint identifier: UUID (player, entity) or free name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WaypointId {
    Uuid(u128),
    Name(String),
}

/// Position of a waypoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaypointPos {
    /// No position (removal).
    Empty,
    /// Exact block.
    Block([i32; 3]),
    /// Chunk (x, z).
    Chunk([i32; 2]),
    /// Direction only (radians).
    Azimuth(f32),
}

/// Operation: track, untrack, update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaypointOp {
    Track,
    Untrack,
    Update,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    pub op: WaypointOp,
    pub id: WaypointId,
    /// Style (`minecraft:default`, `minecraft:bowtie`…).
    pub style: String,
    /// Imposed RGB colour.
    pub color: Option<u32>,
    pub pos: WaypointPos,
}

pub fn read_waypoint(r: &mut Reader<'_>) -> Result<Waypoint> {
    // Operation: identifier folded onto the three values.
    let op = match read_varint(r)?.rem_euclid(3) {
        0 => WaypointOp::Track,
        1 => WaypointOp::Untrack,
        _ => WaypointOp::Update,
    };
    let id = if bool::decode(r)? {
        WaypointId::Uuid(u128::decode(r)?)
    } else {
        WaypointId::Name(rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT)?.to_owned())
    };
    let style = rc_proto::read_str(r, rc_proto::STRING_MAX_DEFAULT)?.to_owned();
    let color = if bool::decode(r)? {
        let (red, green, blue) = (u8::decode(r)?, u8::decode(r)?, u8::decode(r)?);
        Some(u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue))
    } else {
        None
    };
    let pos = match read_varint(r)? {
        1 => WaypointPos::Block([read_varint(r)?, read_varint(r)?, read_varint(r)?]),
        2 => WaypointPos::Chunk([read_varint(r)?, read_varint(r)?]),
        3 => WaypointPos::Azimuth(f32::decode(r)?),
        _ => WaypointPos::Empty,
    };
    Ok(Waypoint { op, id, style, color, pos })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_proto::Encode;
    use rc_proto::varint::write_varint;

    #[test]
    fn track_player_position() {
        let mut w = Vec::new();
        write_varint(&mut w, 0);
        true.encode(&mut w);
        0x1234_u128.encode(&mut w);
        rc_proto::write_str(&mut w, "minecraft:default");
        true.encode(&mut w);
        w.extend_from_slice(&[0xff, 0x80, 0x00]);
        write_varint(&mut w, 1);
        for v in [10, -60, -300] {
            write_varint(&mut w, v);
        }
        let p = read_waypoint(&mut Reader::new(&w)).unwrap();
        assert_eq!(p.op, WaypointOp::Track);
        assert_eq!(p.id, WaypointId::Uuid(0x1234));
        assert_eq!(p.style, "minecraft:default");
        assert_eq!(p.color, Some(0xff8000));
        assert_eq!(p.pos, WaypointPos::Block([10, -60, -300]));
    }
}
