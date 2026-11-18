//! Variable-length integers: 7 data bits per byte, high bit = continuation.
//! Negative values are encoded on their unsigned representation (5 or 10 bytes).

use crate::{Error, Reader, Result};

pub const VARINT_MAX_BYTES: usize = 5;
pub const VARLONG_MAX_BYTES: usize = 10;

pub fn read_varint(r: &mut Reader<'_>) -> Result<i32> {
    let mut value: u32 = 0;
    for i in 0..VARINT_MAX_BYTES {
        let b = r.u8()?;
        value |= u32::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(value as i32);
        }
    }
    Err(Error::VarIntTooLong { max: VARINT_MAX_BYTES })
}

pub fn read_varlong(r: &mut Reader<'_>) -> Result<i64> {
    let mut value: u64 = 0;
    for i in 0..VARLONG_MAX_BYTES {
        let b = r.u8()?;
        value |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(value as i64);
        }
    }
    Err(Error::VarIntTooLong { max: VARLONG_MAX_BYTES })
}

pub fn write_varint(w: &mut Vec<u8>, v: i32) {
    let mut v = v as u32;
    loop {
        if v & !0x7f == 0 {
            w.push(v as u8);
            return;
        }
        w.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
}

pub fn write_varlong(w: &mut Vec<u8>, v: i64) {
    let mut v = v as u64;
    loop {
        if v & !0x7f == 0 {
            w.push(v as u8);
            return;
        }
        w.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
}

/// Number of bytes `v` will take once encoded.
pub const fn varint_len(v: i32) -> usize {
    let v = v as u32;
    if v == 0 {
        return 1;
    }
    let bits = 32 - v.leading_zeros() as usize;
    bits.div_ceil(7)
}

/// Incremental read of a VarInt at the head of a buffer, for network framing.
/// `Ok(None)`: not enough bytes yet; `Ok(Some((value, length)))` otherwise.
pub fn peek_varint(buf: &[u8]) -> Result<Option<(i32, usize)>> {
    let mut value: u32 = 0;
    for i in 0..VARINT_MAX_BYTES {
        let Some(&b) = buf.get(i) else {
            return Ok(None);
        };
        value |= u32::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(Some((value as i32, i + 1)));
        }
    }
    Err(Error::VarIntTooLong { max: VARINT_MAX_BYTES })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values: unsigned LEB128 encoding on 32/64 bits.
    const CASES: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (255, &[0xff, 0x01]),
        (25565, &[0xdd, 0xc7, 0x01]),
        (2097151, &[0xff, 0xff, 0x7f]),
        (i32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x07]),
        (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    #[test]
    fn varint_round_trip() {
        for &(v, bytes) in CASES {
            let mut w = Vec::new();
            write_varint(&mut w, v);
            assert_eq!(w, bytes, "encoding of {v}");
            assert_eq!(varint_len(v), bytes.len(), "length of {v}");
            let mut r = Reader::new(bytes);
            assert_eq!(read_varint(&mut r).unwrap(), v);
            assert!(r.is_empty());
            assert_eq!(peek_varint(bytes).unwrap(), Some((v, bytes.len())));
        }
    }

    #[test]
    fn varlong_round_trip() {
        let cases: &[(i64, &[u8])] = &[
            (0, &[0x00]),
            (2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
            (i64::MAX, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
            (i64::MIN, &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
        ];
        for &(v, bytes) in cases {
            let mut w = Vec::new();
            write_varlong(&mut w, v);
            assert_eq!(w, bytes, "encoding of {v}");
            assert_eq!(read_varlong(&mut Reader::new(bytes)).unwrap(), v);
        }
    }

    #[test]
    fn varint_errors() {
        let too_long = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        assert_eq!(read_varint(&mut Reader::new(&too_long)), Err(Error::VarIntTooLong { max: 5 }));
        assert!(peek_varint(&too_long).is_err());
        assert_eq!(read_varint(&mut Reader::new(&[0x80])), Err(Error::UnexpectedEof { needed: 1 }));
        assert_eq!(peek_varint(&[0x80, 0x80]).unwrap(), None);
    }
}
