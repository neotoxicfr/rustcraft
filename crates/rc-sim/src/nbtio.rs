//! File NBT (`anvil-world.md` § 1): named root, gzip compression of `.dat`, tolerant
//! reading (gzip, otherwise zlib, otherwise raw).

use std::io::{Read, Write};
use std::path::Path;

use rc_proto::Reader;
use rc_proto::nbt::{self, Nbt};

use crate::Error;

/// Decompresses according to the signature: gzip (`1f 8b`), zlib (`78`), otherwise raw.
pub fn decompress_auto(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len() * 4);
    match bytes {
        [0x1f, 0x8b, ..] => flate2::read::MultiGzDecoder::new(bytes).read_to_end(&mut out).map(|_| out),
        [0x78, ..] => flate2::read::ZlibDecoder::new(bytes).read_to_end(&mut out).map(|_| out),
        _ => Ok(bytes.to_vec()),
    }
}

/// Root compound of an uncompressed file NBT; the root name is ignored.
pub fn parse_root(bytes: &[u8]) -> Result<Nbt, Error> {
    let mut r = Reader::new(bytes);
    match nbt::read_named(&mut r) {
        Ok(Some((_, v @ Nbt::Compound(_)))) => Ok(v),
        Ok(_) => Err(Error::Format("NBT root is not a compound".into())),
        Err(e) => Err(Error::Format(format!("NBT : {e}"))),
    }
}

/// Empty named root, uncompressed.
pub fn encode_root(v: &Nbt) -> Vec<u8> {
    let mut w = Vec::with_capacity(4096);
    nbt::write_named(&mut w, "", v);
    w
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::with_capacity(bytes.len() / 2), flate2::Compression::default());
    // Writing into a Vec cannot fail.
    let _ = e.write_all(bytes);
    e.finish().unwrap_or_default()
}

pub fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(bytes.len() / 2), flate2::Compression::default());
    let _ = e.write_all(bytes);
    e.finish().unwrap_or_default()
}

/// Reads a `.dat` file (gzip in practice).
pub fn read_dat(path: &Path) -> Result<Nbt, Error> {
    let raw = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let bytes = decompress_auto(&raw).map_err(|e| Error::io(path, e))?;
    parse_root(&bytes).map_err(|e| Error::Format(format!("{} : {e}", path.display())))
}

/// Field of a compound, modifiable.
pub fn get_mut<'a>(v: &'a mut Nbt, key: &str) -> Option<&'a mut Nbt> {
    match v {
        Nbt::Compound(fields) => fields.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// Replaces (in place, order kept) or adds a field.
pub fn set(v: &mut Nbt, key: &str, value: Nbt) {
    if let Nbt::Compound(fields) = v {
        match fields.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => fields.push((key.to_owned(), value)),
        }
    }
}

pub fn remove(v: &mut Nbt, key: &str) {
    if let Nbt::Compound(fields) = v {
        fields.retain(|(k, _)| k != key);
    }
}

pub fn list(v: Option<&Nbt>) -> &[Nbt] {
    match v {
        Some(Nbt::List(l)) => l,
        _ => &[],
    }
}

/// Heterogeneous list element (§ 1): compound with a single entry of empty name unwrapped.
pub fn unwrap_hetero(v: &Nbt) -> &Nbt {
    match v {
        Nbt::Compound(f) if f.len() == 1 && f[0].0.is_empty() => &f[0].1,
        _ => v,
    }
}

pub fn f64s<const N: usize>(v: Option<&Nbt>) -> Option<[f64; N]> {
    let l = list(v);
    (l.len() == N).then(|| std::array::from_fn(|i| if let Nbt::Double(d) = l[i] { d } else { 0.0 }))
}

/// `Motion` velocity read like the game: a component beyond 10 (in absolute value) is 0.
pub fn motion(v: Option<&Nbt>) -> Option<[f64; 3]> {
    f64s::<3>(v).map(|m| m.map(|c| if c.abs() > 10.0 { 0.0 } else { c }))
}

pub fn f32s<const N: usize>(v: Option<&Nbt>) -> Option<[f32; N]> {
    let l = list(v);
    (l.len() == N).then(|| std::array::from_fn(|i| if let Nbt::Float(d) = l[i] { d } else { 0.0 }))
}

pub fn int(v: &Nbt, key: &str) -> Option<i64> {
    v.get(key).and_then(Nbt::as_i64)
}

pub fn float(v: &Nbt, key: &str) -> Option<f32> {
    match v.get(key)? {
        Nbt::Float(f) => Some(*f),
        Nbt::Double(d) => Some(*d as f32),
        o => o.as_i64().map(|i| i as f32),
    }
}

/// UUID as `int[4]`, most significant first.
pub fn uuid(v: Option<&Nbt>) -> Option<u128> {
    match v? {
        Nbt::IntArray(a) if a.len() == 4 => Some(a.iter().fold(0u128, |acc, &x| (acc << 32) | u128::from(x as u32))),
        _ => None,
    }
}

pub fn uuid_nbt(u: u128) -> Nbt {
    Nbt::IntArray((0..4).map(|i| (u >> (96 - 32 * i)) as u32 as i32).collect())
}

/// UUID as text with dashes (player file names).
pub fn uuid_text(u: u128) -> String {
    let h = format!("{u:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_forms() {
        let u = 0x6c36f763_6526_3b48_9820_8f008743cbebu128;
        assert_eq!(uuid(Some(&uuid_nbt(u))), Some(u));
        assert_eq!(uuid_text(u), "6c36f763-6526-3b48-9820-8f008743cbeb");
    }

    #[test]
    fn auto_detects_compression() {
        let v = Nbt::Compound(vec![("a".into(), Nbt::Int(3))]);
        let raw = encode_root(&v);
        for bytes in [gzip(&raw), zlib(&raw), raw.clone()] {
            assert_eq!(parse_root(&decompress_auto(&bytes).unwrap()).unwrap(), v);
        }
    }
}
