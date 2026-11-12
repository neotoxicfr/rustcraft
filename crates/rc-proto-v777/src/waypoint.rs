//! `waypoint` packet (waypoints of the locator bar, `docs/specs/hud.md` § 4.8).

use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{Decode, Encode, Reader, Result};

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

/// Body of a `waypoint` packet, the exact inverse of [`read_waypoint`] (integrated server
/// talking to a LAN guest).
pub fn write_waypoint(w: &mut Vec<u8>, p: &Waypoint) {
    let op = match p.op {
        WaypointOp::Track => 0,
        WaypointOp::Untrack => 1,
        WaypointOp::Update => 2,
    };
    write_varint(w, op);
    match &p.id {
        WaypointId::Uuid(u) => {
            true.encode(w);
            u.encode(w);
        }
        WaypointId::Name(n) => {
            false.encode(w);
            rc_proto::write_str(w, n);
        }
    }
    rc_proto::write_str(w, &p.style);
    p.color.is_some().encode(w);
    if let Some(c) = p.color {
        w.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
    }
    match p.pos {
        WaypointPos::Empty => write_varint(w, 0),
        WaypointPos::Block(b) => {
            write_varint(w, 1);
            b.iter().for_each(|&v| write_varint(w, v));
        }
        WaypointPos::Chunk(c) => {
            write_varint(w, 2);
            c.iter().for_each(|&v| write_varint(w, v));
        }
        WaypointPos::Azimuth(a) => {
            write_varint(w, 3);
            a.encode(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The writer gives back the same bytes.
        let mut again = Vec::new();
        write_waypoint(&mut again, &p);
        assert_eq!(again, w);
    }

    /// Every kind of position read back as written; the removal is the game's empty waypoint
    /// (default style, no colour, kind 0) and nothing after it.
    #[test]
    fn writer_round_trips_every_kind() {
        let base = Waypoint {
            op: WaypointOp::Update,
            id: WaypointId::Uuid(0x0123_4567_89ab_cdef_0011_2233_4455_6677),
            style: "minecraft:bowtie".into(),
            color: None,
            pos: WaypointPos::Empty,
        };
        let cases = [
            Waypoint { pos: WaypointPos::Chunk([-3, 70_000]), color: Some(0x30_3030), ..base.clone() },
            Waypoint { pos: WaypointPos::Azimuth(-2.5), op: WaypointOp::Track, ..base.clone() },
            Waypoint { id: WaypointId::Name("spot".into()), pos: WaypointPos::Block([0, 1, 2]), ..base.clone() },
            Waypoint { op: WaypointOp::Untrack, style: "minecraft:default".into(), ..base.clone() },
        ];
        for p in cases {
            let mut w = Vec::new();
            write_waypoint(&mut w, &p);
            let mut r = Reader::new(&w);
            assert_eq!(read_waypoint(&mut r).unwrap(), p);
            assert_eq!(r.remaining(), 0, "bytes left after {p:?}");
        }
        let mut removal = Vec::new();
        write_waypoint(&mut removal, &Waypoint { op: WaypointOp::Untrack, ..base });
        assert_eq!(removal[0], 1, "untrack is operation 1");
        assert_eq!(*removal.last().unwrap(), 0, "empty waypoint is kind 0 with no contents");
    }
}
