//! World border packets (`weather-border.md` § 4.1).

use rc_proto::varint::{read_varint, read_varlong};
use rc_proto::{Decode, Reader, Result};

/// A border packet, fields in network order (durations in ticks).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderPacket {
    Init { x: f64, z: f64, old_size: f64, new_size: f64, ticks: i64, max_size: i32, warn_blocks: i32, warn_ticks: i32 },
    Center { x: f64, z: f64 },
    Lerp { old_size: f64, new_size: f64, ticks: i64 },
    Size(f64),
    WarningDelay(i32),
    WarningDistance(i32),
}

pub fn read_init(r: &mut Reader<'_>) -> Result<BorderPacket> {
    Ok(BorderPacket::Init {
        x: f64::decode(r)?,
        z: f64::decode(r)?,
        old_size: f64::decode(r)?,
        new_size: f64::decode(r)?,
        ticks: read_varlong(r)?,
        max_size: read_varint(r)?,
        warn_blocks: read_varint(r)?,
        warn_ticks: read_varint(r)?,
    })
}

pub fn read_center(r: &mut Reader<'_>) -> Result<BorderPacket> {
    Ok(BorderPacket::Center { x: f64::decode(r)?, z: f64::decode(r)? })
}

pub fn read_lerp(r: &mut Reader<'_>) -> Result<BorderPacket> {
    Ok(BorderPacket::Lerp { old_size: f64::decode(r)?, new_size: f64::decode(r)?, ticks: read_varlong(r)? })
}

pub fn read_size(r: &mut Reader<'_>) -> Result<BorderPacket> {
    Ok(BorderPacket::Size(f64::decode(r)?))
}

pub fn read_warning_delay(r: &mut Reader<'_>) -> Result<BorderPacket> {
    Ok(BorderPacket::WarningDelay(read_varint(r)?))
}

pub fn read_warning_distance(r: &mut Reader<'_>) -> Result<BorderPacket> {
    Ok(BorderPacket::WarningDistance(read_varint(r)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_fields_in_order() {
        let mut w = Vec::new();
        for v in [1.5f64, -2.0, 100.0, 50.0] {
            w.extend_from_slice(&v.to_be_bytes());
        }
        rc_proto::varint::write_varlong(&mut w, 400);
        for v in [29_999_984, 5, 15] {
            rc_proto::varint::write_varint(&mut w, v);
        }
        let p = read_init(&mut Reader::new(&w)).unwrap();
        assert_eq!(
            p,
            BorderPacket::Init {
                x: 1.5,
                z: -2.0,
                old_size: 100.0,
                new_size: 50.0,
                ticks: 400,
                max_size: 29_999_984,
                warn_blocks: 5,
                warn_ticks: 15
            }
        );
    }
}
