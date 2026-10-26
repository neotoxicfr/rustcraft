use crate::varint::{read_varint, write_varint};
use crate::{Error, Reader, Result, STRING_MAX_DEFAULT};

/// Decoding from a `Reader`. The lifetime `'a` allows returning borrowed views
/// (`&'a str`, `&'a [u8]`) without copying.
pub trait Decode<'a>: Sized {
    fn decode(r: &mut Reader<'a>) -> Result<Self>;
}

/// Encoding at the end of a `Vec<u8>`. Infallible: bounds are checked when values are
/// constructed, not on writing.
pub trait Encode {
    fn encode(&self, w: &mut Vec<u8>);
}

macro_rules! be_int {
    ($($t:ty),*) => {$(
        impl<'a> Decode<'a> for $t {
            #[inline]
            fn decode(r: &mut Reader<'a>) -> Result<Self> {
                Ok(<$t>::from_be_bytes(r.take_array()?))
            }
        }
        impl Encode for $t {
            #[inline]
            fn encode(&self, w: &mut Vec<u8>) {
                w.extend_from_slice(&self.to_be_bytes());
            }
        }
    )*};
}
be_int!(u8, i8, u16, i16, u32, i32, u64, i64, u128, f32, f64);

impl<'a> Decode<'a> for bool {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        match r.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(Error::InvalidBool(b)),
        }
    }
}

impl Encode for bool {
    fn encode(&self, w: &mut Vec<u8>) {
        w.push(u8::from(*self));
    }
}

/// Reads a prefixed length (VarInt) and bounds it.
pub fn read_len(r: &mut Reader<'_>, max: usize) -> Result<usize> {
    let len = read_varint(r)?;
    if len < 0 || len as usize > max {
        return Err(Error::LengthOutOfRange { len: i64::from(len), max });
    }
    Ok(len as usize)
}

/// Number of elements of a list each taking at least `elem_min_bytes` on the network:
/// a hostile server cannot make us reserve more than the packet can hold.
pub fn read_count(r: &mut Reader<'_>, elem_min_bytes: usize) -> Result<usize> {
    let len = read_varint(r)?;
    let max = r.remaining() / elem_min_bytes.max(1);
    if len < 0 || len as usize > max {
        return Err(Error::LengthOutOfRange { len: i64::from(len), max });
    }
    Ok(len as usize)
}

pub fn write_len(w: &mut Vec<u8>, len: usize) {
    debug_assert!(len <= i32::MAX as usize);
    write_varint(w, len as i32);
}

/// String bounded to `max` UTF-16 units (the protocol counts in UTF-16 on the official side).
pub fn read_str<'a>(r: &mut Reader<'a>, max: usize) -> Result<&'a str> {
    // A UTF-16 character takes at most 3 bytes in UTF-8.
    let len = read_len(r, max * 3)?;
    let s = std::str::from_utf8(r.take(len)?).map_err(|_| Error::InvalidUtf8)?;
    // As long as len ≤ max, the number of UTF-16 units cannot exceed max: we skip the count.
    if len > max {
        let units = s.encode_utf16().count();
        if units > max {
            return Err(Error::LengthOutOfRange { len: units as i64, max });
        }
    }
    Ok(s)
}

pub fn write_str(w: &mut Vec<u8>, s: &str) {
    write_len(w, s.len());
    w.extend_from_slice(s.as_bytes());
}

impl<'a> Decode<'a> for &'a str {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        read_str(r, STRING_MAX_DEFAULT)
    }
}

impl Encode for &str {
    fn encode(&self, w: &mut Vec<u8>) {
        write_str(w, self);
    }
}

impl<'a> Decode<'a> for String {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        read_str(r, STRING_MAX_DEFAULT).map(str::to_owned)
    }
}

impl Encode for String {
    fn encode(&self, w: &mut Vec<u8>) {
        write_str(w, self);
    }
}

/// Optional field prefixed by a boolean.
impl<'a, T: Decode<'a>> Decode<'a> for Option<T> {
    fn decode(r: &mut Reader<'a>) -> Result<Self> {
        if bool::decode(r)? { T::decode(r).map(Some) } else { Ok(None) }
    }
}

impl<T: Encode> Encode for Option<T> {
    fn encode(&self, w: &mut Vec<u8>) {
        match self {
            Some(v) => {
                true.encode(w);
                v.encode(w);
            }
            None => false.encode(w),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_is_capped_by_minimum_element_size() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 5);
        buf.extend([0u8; 10]);
        assert_eq!(read_count(&mut Reader::new(&buf), 2).unwrap(), 5);
        assert!(read_count(&mut Reader::new(&buf), 3).is_err());
        let mut neg = Vec::new();
        write_varint(&mut neg, -1);
        assert!(read_count(&mut Reader::new(&neg), 1).is_err());
    }

    fn round_trip<T>(v: T) -> T
    where
        T: Encode + for<'a> Decode<'a>,
    {
        let mut w = Vec::new();
        v.encode(&mut w);
        let mut r = Reader::new(&w);
        let out = T::decode(&mut r).unwrap();
        r.finish().unwrap();
        out
    }

    #[test]
    fn ints_are_big_endian() {
        let mut w = Vec::new();
        0x0102_0304_i32.encode(&mut w);
        25565_u16.encode(&mut w);
        assert_eq!(w, [1, 2, 3, 4, 0x63, 0xdd]);
        assert_eq!(round_trip(-12345_i64), -12345);
        assert_eq!(round_trip(1.5_f64), 1.5);
        assert_eq!(round_trip(u128::MAX - 7), u128::MAX - 7);
    }

    #[test]
    fn bools() {
        assert!(round_trip(true));
        assert_eq!(bool::decode(&mut Reader::new(&[2])), Err(Error::InvalidBool(2)));
    }

    #[test]
    fn strings() {
        assert_eq!(round_trip(String::from("été ☃ 𝄞")), "été ☃ 𝄞");
        let mut w = Vec::new();
        write_str(&mut w, "abc");
        assert_eq!(w, [3, b'a', b'b', b'c']);
        // Borrowed view: points into the buffer.
        let mut r = Reader::new(&w);
        let s: &str = Decode::decode(&mut r).unwrap();
        assert_eq!(s.as_ptr(), w[1..].as_ptr());
    }

    #[test]
    fn string_limits_count_utf16_units() {
        // 4 non-BMP characters = 8 UTF-16 units, 16 bytes.
        let s = "𝄞𝄞𝄞𝄞";
        let mut w = Vec::new();
        write_str(&mut w, s);
        assert!(read_str(&mut Reader::new(&w), 8).is_ok());
        assert_eq!(read_str(&mut Reader::new(&w), 7), Err(Error::LengthOutOfRange { len: 8, max: 7 }));
        // Length in bytes beyond max*3: rejected before reading.
        assert!(matches!(read_str(&mut Reader::new(&w), 4), Err(Error::LengthOutOfRange { len: 16, max: 12 })));
        let bad = [2, 0xc3, 0x28];
        assert_eq!(read_str(&mut Reader::new(&bad), 10), Err(Error::InvalidUtf8));
    }

    #[test]
    fn options() {
        assert_eq!(round_trip(Some(7_u8)), Some(7));
        assert_eq!(round_trip(None::<u8>), None);
    }
}
