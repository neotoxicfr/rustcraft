use crate::codec::{read_str, write_str};
use crate::varint::{read_varint, read_varlong, write_varint, write_varlong};
use crate::{Decode, Encode, Error, Reader, Result};

/// 32-bit integer encoded with variable length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct VarInt(pub i32);

/// 64-bit integer encoded with variable length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct VarLong(pub i64);

impl<'a> Decode<'a> for VarInt {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        read_varint(r).map(Self)
    }
}

impl Encode for VarInt {
    fn encode(&self, w: &mut Vec<u8>) {
        write_varint(w, self.0);
    }
}

impl<'a> Decode<'a> for VarLong {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        read_varlong(r).map(Self)
    }
}

impl Encode for VarLong {
    fn encode(&self, w: &mut Vec<u8>) {
        write_varlong(w, self.0);
    }
}

/// String with a field-specific bound (nickname: 16, identifier: 32767…), in UTF-16 units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bounded<T, const MAX: usize>(pub T);

impl<'a, const MAX: usize> Decode<'a> for Bounded<&'a str, MAX> {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        read_str(r, MAX).map(Self)
    }
}

impl<const MAX: usize> Encode for Bounded<&str, MAX> {
    fn encode(&self, w: &mut Vec<u8>) {
        write_str(w, self.0);
    }
}

impl<'a, const MAX: usize> Decode<'a> for Bounded<String, MAX> {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        read_str(r, MAX).map(|s| Self(s.to_owned()))
    }
}

impl<const MAX: usize> Encode for Bounded<String, MAX> {
    fn encode(&self, w: &mut Vec<u8>) {
        write_str(w, &self.0);
    }
}

/// Block position packed into 64 bits: x on 26 bits, z on 26, y on 12 (signed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const X_MAX: i32 = (1 << 25) - 1;
    pub const Y_MAX: i32 = (1 << 11) - 1;

    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub const fn pack(self) -> i64 {
        ((self.x as i64 & 0x3ff_ffff) << 38) | ((self.z as i64 & 0x3ff_ffff) << 12) | (self.y as i64 & 0xfff)
    }

    pub const fn unpack(v: i64) -> Self {
        Self { x: (v >> 38) as i32, y: ((v << 52) >> 52) as i32, z: ((v << 26) >> 38) as i32 }
    }

    /// True if the position fits the packed format without loss.
    pub const fn is_packable(self) -> bool {
        self.x >= -Self::X_MAX - 1
            && self.x <= Self::X_MAX
            && self.z >= -Self::X_MAX - 1
            && self.z <= Self::X_MAX
            && self.y >= -Self::Y_MAX - 1
            && self.y <= Self::Y_MAX
    }
}

impl<'a> Decode<'a> for BlockPos {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        i64::decode(r).map(Self::unpack)
    }
}

impl Encode for BlockPos {
    fn encode(&self, w: &mut Vec<u8>) {
        debug_assert!(self.is_packable(), "position outside format: {self:?}");
        self.pack().encode(w);
    }
}

/// Angle in 1/256 of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Angle(pub u8);

impl Angle {
    pub fn from_degrees(deg: f32) -> Self {
        Self((deg * 256.0 / 360.0).floor().rem_euclid(256.0) as u8)
    }

    pub fn degrees(self) -> f32 {
        f32::from(self.0) * 360.0 / 256.0
    }
}

impl<'a> Decode<'a> for Angle {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        r.u8().map(Self)
    }
}

impl Encode for Angle {
    fn encode(&self, w: &mut Vec<u8>) {
        w.push(self.0);
    }
}

/// Checks that a received integer value belongs to a known enumeration.
pub fn check_enum(what: &'static str, value: i32, count: i32) -> Result<i32> {
    if (0..count).contains(&value) { Ok(value) } else { Err(Error::InvalidValue { what, value: i64::from(value) }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_pos_packing() {
        let cases = [
            BlockPos::new(0, 0, 0),
            BlockPos::new(1, 2, 3),
            BlockPos::new(-1, -1, -1),
            BlockPos::new(-64, -64, 300),
            BlockPos::new(BlockPos::X_MAX, BlockPos::Y_MAX, -BlockPos::X_MAX - 1),
            BlockPos::new(-BlockPos::X_MAX - 1, -BlockPos::Y_MAX - 1, BlockPos::X_MAX),
            BlockPos::new(29_999_984, 319, -29_999_984),
        ];
        for p in cases {
            assert!(p.is_packable());
            assert_eq!(BlockPos::unpack(p.pack()), p);
            let mut w = Vec::new();
            p.encode(&mut w);
            assert_eq!(BlockPos::decode(&mut Reader::new(&w)).unwrap(), p);
        }
        // Bit layout: x at the top, z in the middle, y at the bottom.
        assert_eq!(BlockPos::new(1, 0, 0).pack(), 1 << 38);
        assert_eq!(BlockPos::new(0, 0, 1).pack(), 1 << 12);
        assert_eq!(BlockPos::new(0, 1, 0).pack(), 1);
        assert!(!BlockPos::new(0, 2048, 0).is_packable());
    }

    #[test]
    fn angles() {
        assert_eq!(Angle::from_degrees(0.0).0, 0);
        assert_eq!(Angle::from_degrees(90.0).0, 64);
        assert_eq!(Angle::from_degrees(-90.0).0, 192);
        assert_eq!(Angle::from_degrees(360.0).0, 0);
        assert_eq!(Angle(128).degrees(), 180.0);
    }

    #[test]
    fn bounded_strings() {
        let mut w = Vec::new();
        Bounded::<&str, 16>("Steve_Like_Name").encode(&mut w);
        assert!(Bounded::<&str, 16>::decode(&mut Reader::new(&w)).is_ok());
        assert!(Bounded::<&str, 4>::decode(&mut Reader::new(&w)).is_err());
    }

    #[test]
    fn enums() {
        assert_eq!(check_enum("mode", 2, 4), Ok(2));
        assert!(check_enum("mode", 4, 4).is_err());
        assert!(check_enum("mode", -1, 4).is_err());
    }
}
