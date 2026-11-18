//! `map_item_data` packet (data of a map, `docs/specs/maps.md` § 1).

use rc_proto::varint::read_varint;
use rc_proto::{Decode, Reader, Result};

/// Decoration of a map (marker, banner, structure…).
#[derive(Debug, Clone, PartialEq)]
pub struct MapDecoration {
    /// Index in the built-in registry of decoration types.
    pub kind: i32,
    /// Position −128..127 (pixel = x/2 + 64; y toward the bottom of the map).
    pub x: i8,
    pub y: i8,
    /// Rotation in sixteenths of a turn, already brought to 0..15.
    pub rot: u8,
    /// Optional name (text component as network NBT).
    pub name: Option<rc_proto::nbt::Nbt>,
}

/// Updated pixel rectangle: corner, size and bytes (index x + width·y).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapPatch {
    pub x0: u8,
    pub y0: u8,
    pub width: u8,
    pub height: u8,
    pub colors: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapItemData {
    pub id: i32,
    pub scale: i8,
    pub locked: bool,
    /// `None`: the known list is kept.
    pub decorations: Option<Vec<MapDecoration>>,
    pub patch: Option<MapPatch>,
}

pub fn read_map_item_data(r: &mut Reader<'_>) -> Result<MapItemData> {
    let id = read_varint(r)?;
    let scale = i8::decode(r)?;
    let locked = bool::decode(r)?;
    let decorations = if bool::decode(r)? {
        // A decoration takes at least 5 bytes (type, x, y, rotation, name presence).
        let n = rc_proto::read_count(r, 5)?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let kind = read_varint(r)?;
            let x = i8::decode(r)?;
            let y = i8::decode(r)?;
            let rot = (i8::decode(r)? & 15) as u8;
            let name = if bool::decode(r)? { rc_proto::nbt::read(r)? } else { None };
            v.push(MapDecoration { kind, x, y, rot, name });
        }
        Some(v)
    } else {
        None
    };
    let width = u8::decode(r)?;
    let patch = if width == 0 {
        None
    } else {
        let height = u8::decode(r)?;
        let x0 = u8::decode(r)?;
        let y0 = u8::decode(r)?;
        let n = rc_proto::read_len(r, r.remaining())?;
        let colors = r.take(n)?.to_vec();
        Some(MapPatch { x0, y0, width, height, colors })
    };
    Ok(MapItemData { id, scale, locked, decorations, patch })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_packet() {
        // id 7, scale 2, not locked, 1 decoration (type 1, x −4, y 10, rot 0x13 → 3, without
        // name), 2×1 rectangle at (5, 6).
        let b = [7, 2, 0, 1, 1, 1, 0xfc, 10, 0x13, 0, 2, 1, 5, 6, 2, 0x1d, 0x22];
        let mut r = Reader::new(&b);
        let m = read_map_item_data(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!((m.id, m.scale, m.locked), (7, 2, false));
        let d = m.decorations.unwrap();
        assert_eq!((d[0].kind, d[0].x, d[0].y, d[0].rot, d[0].name.is_none()), (1, -4, 10, 3, true));
        assert_eq!(m.patch.unwrap(), MapPatch { x0: 5, y0: 6, width: 2, height: 1, colors: vec![0x1d, 0x22] });
    }

    #[test]
    fn no_decorations_no_patch() {
        let b = [3, 0, 1, 0, 0];
        let m = read_map_item_data(&mut Reader::new(&b)).unwrap();
        assert_eq!((m.id, m.locked, m.decorations, m.patch), (3, true, None, None));
    }
}
