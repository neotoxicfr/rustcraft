//! Network NBT: nameless root tag (type byte then payload), type 0 = absent.
//! Strings in modified UTF-8 prefixed by a `u16`. See `docs/specs/chunks.md`.

use crate::{Error, Reader, Result};

/// Maximum accepted nesting depth (protection against hostile data).
pub const MAX_DEPTH: usize = 512;

#[derive(Debug, Clone, PartialEq)]
pub enum Nbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Nbt>),
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    pub fn type_id(&self) -> u8 {
        match self {
            Nbt::Byte(_) => 1,
            Nbt::Short(_) => 2,
            Nbt::Int(_) => 3,
            Nbt::Long(_) => 4,
            Nbt::Float(_) => 5,
            Nbt::Double(_) => 6,
            Nbt::ByteArray(_) => 7,
            Nbt::String(_) => 8,
            Nbt::List(_) => 9,
            Nbt::Compound(_) => 10,
            Nbt::IntArray(_) => 11,
            Nbt::LongArray(_) => 12,
        }
    }

    /// Field of a compound.
    pub fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Integer value, whatever the tag's width.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Nbt::Byte(v) => Some(v.into()),
            Nbt::Short(v) => Some(v.into()),
            Nbt::Int(v) => Some(v.into()),
            Nbt::Long(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Nbt::String(s) => Some(s),
            _ => None,
        }
    }
}

fn be<const N: usize>(r: &mut Reader<'_>) -> Result<[u8; N]> {
    r.take_array()
}

fn len_i32(r: &mut Reader<'_>, elem: usize) -> Result<usize> {
    let n = i32::from_be_bytes(be(r)?);
    // Each element takes at least `elem` bytes: we refuse to allocate more than what remains.
    if n < 0 || (n as usize).saturating_mul(elem.max(1)) > r.remaining() {
        return Err(Error::LengthOutOfRange { len: n.into(), max: r.remaining() / elem.max(1) });
    }
    Ok(n as usize)
}

/// Decodes Java's modified UTF-8 (null on 2 bytes, surrogate pairs on 6 bytes).
fn mutf8(bytes: &[u8]) -> Result<String> {
    decode_mutf8(bytes, WriteOptions::from_env().string_fast_path)
}

/// `skip_redundant_scans` (same switch as the string writer): valid UTF-8 is taken as it is
/// without two more scans. 0xC0 never occurs in valid UTF-8, and 0xED there only leads a
/// character of U+D000..U+D7FF (hangul), which the unit decoder below gives back unchanged.
/// Only difference: valid UTF-8 holding both a 0xED lead and a 4-byte character, refused by the
/// old path (the unit decoder knows no 4-byte form), is now accepted like the same text without
/// the 0xED character always was.
fn decode_mutf8(bytes: &[u8], skip_redundant_scans: bool) -> Result<String> {
    if let Ok(s) = std::str::from_utf8(bytes)
        && (skip_redundant_scans || (!bytes.contains(&0xc0) && !bytes.contains(&0xed)))
    {
        return Ok(s.to_owned());
    }
    decode_mutf8_units(bytes)
}

/// Unit-by-unit decoder: null on 2 bytes, surrogate halves on 3 bytes each.
fn decode_mutf8_units(bytes: &[u8]) -> Result<String> {
    let mut units = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let (u, n) = if b < 0x80 {
            (u16::from(b), 1)
        } else if b & 0xe0 == 0xc0 && i + 1 < bytes.len() {
            ((u16::from(b & 0x1f) << 6) | u16::from(bytes[i + 1] & 0x3f), 2)
        } else if b & 0xf0 == 0xe0 && i + 2 < bytes.len() {
            ((u16::from(b & 0x0f) << 12) | (u16::from(bytes[i + 1] & 0x3f) << 6) | u16::from(bytes[i + 2] & 0x3f), 3)
        } else {
            return Err(Error::InvalidUtf8);
        };
        units.push(u);
        i += n;
    }
    String::from_utf16(&units).map_err(|_| Error::InvalidUtf8)
}

fn read_string(r: &mut Reader<'_>) -> Result<String> {
    let n = u16::from_be_bytes(be(r)?) as usize;
    mutf8(r.take(n)?)
}

fn skip_string(r: &mut Reader<'_>) -> Result<()> {
    let n = u16::from_be_bytes(be(r)?) as usize;
    r.take(n).map(|_| ())
}

fn read_payload(r: &mut Reader<'_>, ty: u8, depth: usize) -> Result<Nbt> {
    if depth > MAX_DEPTH {
        return Err(Error::InvalidValue { what: "NBT depth", value: depth as i64 });
    }
    Ok(match ty {
        1 => Nbt::Byte(r.u8()? as i8),
        2 => Nbt::Short(i16::from_be_bytes(be(r)?)),
        3 => Nbt::Int(i32::from_be_bytes(be(r)?)),
        4 => Nbt::Long(i64::from_be_bytes(be(r)?)),
        5 => Nbt::Float(f32::from_be_bytes(be(r)?)),
        6 => Nbt::Double(f64::from_be_bytes(be(r)?)),
        7 => {
            let n = len_i32(r, 1)?;
            Nbt::ByteArray(r.take(n)?.to_vec())
        }
        8 => Nbt::String(read_string(r)?),
        9 => {
            let elem = r.u8()?;
            let n = len_i32(r, 1)?;
            if elem == 0 && n > 0 {
                return Err(Error::InvalidValue { what: "NBT list of end type", value: n as i64 });
            }
            let mut v = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                // Heterogeneous list: the `{"": x}` wrapper is unwrapped on reading (like the game).
                v.push(unwrap_element(read_payload(r, elem, depth + 1)?));
            }
            Nbt::List(v)
        }
        10 => {
            let mut fields = Vec::new();
            loop {
                let t = r.u8()?;
                if t == 0 {
                    break;
                }
                let name = read_string(r)?;
                fields.push((name, read_payload(r, t, depth + 1)?));
            }
            Nbt::Compound(fields)
        }
        11 => {
            let n = len_i32(r, 4)?;
            Nbt::IntArray(r.take(n * 4)?.as_chunks::<4>().0.iter().map(|c| i32::from_be_bytes(*c)).collect())
        }
        12 => {
            let n = len_i32(r, 8)?;
            Nbt::LongArray(r.take(n * 8)?.as_chunks::<8>().0.iter().map(|c| i64::from_be_bytes(*c)).collect())
        }
        t => return Err(Error::InvalidValue { what: "type NBT", value: t.into() }),
    })
}

/// Compound with a single entry of empty name: this is the wrapper of a heterogeneous list element.
fn is_wrapper(fields: &[(String, Nbt)]) -> bool {
    fields.len() == 1 && fields[0].0.is_empty()
}

fn unwrap_element(v: Nbt) -> Nbt {
    match v {
        Nbt::Compound(mut f) if is_wrapper(&f) => f.pop().map_or(Nbt::Compound(Vec::new()), |(_, x)| x),
        o => o,
    }
}

/// Element type written for a list: the common type, 10 (compound) if the types differ, 0 if empty.
fn list_element_type(items: &[Nbt]) -> u8 {
    let Some(first) = items.first() else { return 0 };
    let t = first.type_id();
    if items.iter().all(|i| i.type_id() == t) { t } else { 10 }
}

fn skip_payload(r: &mut Reader<'_>, ty: u8, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::InvalidValue { what: "NBT depth", value: depth as i64 });
    }
    let fixed = match ty {
        1 => 1,
        2 => 2,
        3 | 5 => 4,
        4 | 6 => 8,
        7 => len_i32(r, 1)?,
        11 => len_i32(r, 4)? * 4,
        12 => len_i32(r, 8)? * 8,
        8 => return skip_string(r),
        9 => {
            let elem = r.u8()?;
            let n = len_i32(r, 1)?;
            if elem == 0 && n > 0 {
                return Err(Error::InvalidValue { what: "NBT list of end type", value: n as i64 });
            }
            for _ in 0..n {
                skip_payload(r, elem, depth + 1)?;
            }
            return Ok(());
        }
        10 => loop {
            let t = r.u8()?;
            if t == 0 {
                return Ok(());
            }
            skip_string(r)?;
            skip_payload(r, t, depth + 1)?;
        },
        t => return Err(Error::InvalidValue { what: "type NBT", value: t.into() }),
    };
    r.take(fixed).map(|_| ())
}

/// Reads a network tag; `None` if the type is 0 (absent).
pub fn read(r: &mut Reader<'_>) -> Result<Option<Nbt>> {
    match r.u8()? {
        0 => Ok(None),
        ty => read_payload(r, ty, 0).map(Some),
    }
}

/// Skips a network tag without allocating anything.
pub fn skip(r: &mut Reader<'_>) -> Result<()> {
    match r.u8()? {
        0 => Ok(()),
        ty => skip_payload(r, ty, 0),
    }
}

/// Choices of the NBT writer. [`WriteOptions::from_env`] follows the switches; the reference is
/// the code from before the optimisation, kept for A/B comparisons and equivalence tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteOptions {
    /// Strings that read the same in UTF-8 and in modified UTF-8 are copied as they are, the
    /// others are encoded straight into the output (`RUSTCRAFT_OPT_NBT_STRING_FAST_PATH`; 0 =
    /// every string encoded into a temporary buffer, then copied).
    pub string_fast_path: bool,
    /// Int and long arrays converted in one block into reserved space
    /// (`RUSTCRAFT_OPT_NBT_ARRAY_BULK`; 0 = one element at a time).
    pub array_bulk: bool,
}

impl WriteOptions {
    pub const REFERENCE: WriteOptions = WriteOptions { string_fast_path: false, array_bulk: false };
    pub const OPTIMISED: WriteOptions = WriteOptions { string_fast_path: true, array_bulk: true };

    /// Options chosen by the switches, read once.
    pub fn from_env() -> WriteOptions {
        static OPTIONS: std::sync::OnceLock<WriteOptions> = std::sync::OnceLock::new();
        *OPTIONS.get_or_init(|| WriteOptions {
            string_fast_path: rc_data::env_str!("RUSTCRAFT_OPT_NBT_STRING_FAST_PATH") != Some("0"),
            array_bulk: rc_data::env_str!("RUSTCRAFT_OPT_NBT_ARRAY_BULK") != Some("0"),
        })
    }
}

/// Longest string payload: its length is written on 16 bits.
const MAX_STRING_BYTES: usize = u16::MAX as usize;

fn write_string(w: &mut Vec<u8>, s: &str, options: WriteOptions) {
    if options.string_fast_path {
        write_string_in_place(w, s);
    } else {
        write_string_through_buffer(w, s);
    }
}

/// Appends one UTF-16 unit in modified UTF-8: null on 2 bytes, each half of a surrogate pair
/// on 3 bytes.
fn push_modified_utf8_unit(out: &mut Vec<u8>, u: u16) {
    match u {
        0x0001..=0x007f => out.push(u as u8),
        0x0000 | 0x0080..=0x07ff => {
            out.push(0xc0 | (u >> 6) as u8);
            out.push(0x80 | (u & 0x3f) as u8);
        }
        _ => {
            out.push(0xe0 | (u >> 12) as u8);
            out.push(0x80 | ((u >> 6) & 0x3f) as u8);
            out.push(0x80 | (u & 0x3f) as u8);
        }
    }
}

/// Reference writer: the whole string encoded into a temporary buffer, then copied.
fn write_string_through_buffer(w: &mut Vec<u8>, s: &str) {
    let mut out = Vec::with_capacity(s.len());
    s.encode_utf16().for_each(|u| push_modified_utf8_unit(&mut out, u));
    // Beyond 65,535 bytes, the game writes an empty string instead (fallback of its file
    // stream): the length does not fit in 16 bits.
    if out.len() > MAX_STRING_BYTES {
        w.extend_from_slice(&0u16.to_be_bytes());
        return;
    }
    w.extend_from_slice(&(out.len() as u16).to_be_bytes());
    w.extend_from_slice(&out);
}

/// True when the UTF-8 bytes are already the modified UTF-8 bytes: no null character (2 bytes
/// in modified UTF-8) and no character outside the base plane (its UTF-8 lead byte is 0xF0 or
/// above; modified UTF-8 writes a surrogate pair of 6 bytes). Every other character, hangul
/// included, has the same 1 to 3 bytes in both.
fn is_same_in_modified_utf8(s: &str) -> bool {
    s.bytes().all(|b| b != 0 && b < 0xf0)
}

/// Writer without a temporary buffer: almost every key and value (identifiers, names) is copied
/// as it is; the others are encoded straight after a length patched at the end.
fn write_string_in_place(w: &mut Vec<u8>, s: &str) {
    if is_same_in_modified_utf8(s) {
        // Too long: an empty string, like the reference writer.
        if s.len() > MAX_STRING_BYTES {
            w.extend_from_slice(&0u16.to_be_bytes());
        } else {
            w.extend_from_slice(&(s.len() as u16).to_be_bytes());
            w.extend_from_slice(s.as_bytes());
        }
        return;
    }
    let length_at = w.len();
    w.extend_from_slice(&0u16.to_be_bytes());
    s.encode_utf16().for_each(|u| push_modified_utf8_unit(w, u));
    let written = w.len() - length_at - 2;
    if written > MAX_STRING_BYTES {
        // Too long: back to an empty string (its zero length is already in place).
        w.truncate(length_at + 2);
    } else {
        w[length_at..length_at + 2].copy_from_slice(&(written as u16).to_be_bytes());
    }
}

/// Appends the big-endian bytes of `items` with one resize: the loop over fixed-size blocks then
/// has no capacity check per element, and the compiler turns it into vector byte swaps.
fn extend_big_endian<T: Copy, const N: usize>(w: &mut Vec<u8>, items: &[T], to_be_bytes: impl Fn(T) -> [u8; N]) {
    let start = w.len();
    w.resize(start + items.len() * N, 0);
    let (blocks, _) = w[start..].as_chunks_mut::<N>();
    for (block, &x) in blocks.iter_mut().zip(items) {
        *block = to_be_bytes(x);
    }
}

fn write_int_array(w: &mut Vec<u8>, a: &[i32], options: WriteOptions) {
    w.extend_from_slice(&(a.len() as i32).to_be_bytes());
    if options.array_bulk {
        extend_big_endian(w, a, i32::to_be_bytes);
    } else {
        a.iter().for_each(|x| w.extend_from_slice(&x.to_be_bytes()));
    }
}

fn write_long_array(w: &mut Vec<u8>, a: &[i64], options: WriteOptions) {
    w.extend_from_slice(&(a.len() as i32).to_be_bytes());
    if options.array_bulk {
        extend_big_endian(w, a, i64::to_be_bytes);
    } else {
        a.iter().for_each(|x| w.extend_from_slice(&x.to_be_bytes()));
    }
}

fn write_payload(w: &mut Vec<u8>, v: &Nbt, options: WriteOptions) {
    match v {
        Nbt::Byte(x) => w.push(*x as u8),
        Nbt::Short(x) => w.extend_from_slice(&x.to_be_bytes()),
        Nbt::Int(x) => w.extend_from_slice(&x.to_be_bytes()),
        Nbt::Long(x) => w.extend_from_slice(&x.to_be_bytes()),
        Nbt::Float(x) => w.extend_from_slice(&x.to_be_bytes()),
        Nbt::Double(x) => w.extend_from_slice(&x.to_be_bytes()),
        Nbt::ByteArray(a) => {
            w.extend_from_slice(&(a.len() as i32).to_be_bytes());
            w.extend_from_slice(a);
        }
        Nbt::String(s) => write_string(w, s, options),
        Nbt::List(items) => {
            let ty = list_element_type(items);
            w.push(ty);
            w.extend_from_slice(&(items.len() as i32).to_be_bytes());
            for i in items {
                // List of compounds (or mixed): a non-compound element, or a compound that
                // itself has the shape of a wrapper, is wrapped in `{"": x}`.
                let wrap = ty == 10
                    && match i {
                        Nbt::Compound(f) => is_wrapper(f),
                        _ => true,
                    };
                if wrap {
                    w.push(i.type_id());
                    write_string(w, "", options);
                    write_payload(w, i, options);
                    w.push(0);
                } else {
                    write_payload(w, i, options);
                }
            }
        }
        Nbt::Compound(fields) => {
            for (k, v) in fields {
                w.push(v.type_id());
                write_string(w, k, options);
                write_payload(w, v, options);
            }
            w.push(0);
        }
        Nbt::IntArray(a) => write_int_array(w, a, options),
        Nbt::LongArray(a) => write_long_array(w, a, options),
    }
}

/// Writes a network tag (type 0 if absent).
pub fn write(w: &mut Vec<u8>, v: Option<&Nbt>) {
    write_with(w, v, WriteOptions::from_env());
}

/// [`write`] with explicit writer options (A/B comparisons).
pub fn write_with(w: &mut Vec<u8>, v: Option<&Nbt>, options: WriteOptions) {
    match v {
        None => w.push(0),
        Some(v) => {
            w.push(v.type_id());
            write_payload(w, v, options);
        }
    }
}

/// Reads a file root tag (type, name, payload); returns its name and value.
pub fn read_named(r: &mut Reader<'_>) -> Result<Option<(String, Nbt)>> {
    match r.u8()? {
        0 => Ok(None),
        ty => {
            let name = read_string(r)?;
            read_payload(r, ty, 0).map(|v| Some((name, v)))
        }
    }
}

/// Writes a file root tag (type, name, payload).
pub fn write_named(w: &mut Vec<u8>, name: &str, v: &Nbt) {
    write_named_with(w, name, v, WriteOptions::from_env());
}

/// [`write_named`] with explicit writer options (A/B comparisons).
pub fn write_named_with(w: &mut Vec<u8>, name: &str, v: &Nbt, options: WriteOptions) {
    w.push(v.type_id());
    write_string(w, name, options);
    write_payload(w, v, options);
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn sample() -> Nbt {
        Nbt::Compound(vec![
            ("min_y".into(), Nbt::Int(-64)),
            ("height".into(), Nbt::Int(384)),
            ("name".into(), Nbt::String("été\0𝄞".into())),
            ("l".into(), Nbt::List(vec![Nbt::Short(1), Nbt::Short(2)])),
            ("empty".into(), Nbt::List(vec![])),
            ("la".into(), Nbt::LongArray(vec![i64::MIN, 0, 7])),
            ("ia".into(), Nbt::IntArray(vec![1, -1])),
            ("ba".into(), Nbt::ByteArray(vec![1, 2, 3])),
            ("c".into(), Nbt::Compound(vec![("d".into(), Nbt::Double(0.5)), ("f".into(), Nbt::Float(1.5))])),
            ("b".into(), Nbt::Byte(-3)),
            ("long".into(), Nbt::Long(1 << 40)),
        ])
    }

    #[test]
    fn round_trip_and_skip() {
        let v = sample();
        let mut w = Vec::new();
        write(&mut w, Some(&v));
        w.extend_from_slice(&[0xaa]); // next byte, must not be consumed
        let mut r = Reader::new(&w);
        assert_eq!(read(&mut r).unwrap(), Some(v.clone()));
        assert_eq!(r.rest(), [0xaa]);
        let mut r = Reader::new(&w);
        skip(&mut r).unwrap();
        assert_eq!(r.rest(), [0xaa]);
        assert_eq!(v.get("height").and_then(Nbt::as_i64), Some(384));
        assert_eq!(v.get("name").and_then(Nbt::as_str), Some("été\0𝄞"));
    }

    /// Expected bytes frozen, for both string writers: null on 2 bytes, a character outside the
    /// base plane as a surrogate pair (2 × 3 bytes), hangul and Latin-1 as in UTF-8.
    #[test]
    fn modified_utf8_encoding() {
        let cases: [(&str, &[u8]); 6] = [
            ("", &[0, 0]),
            ("a", &[0, 1, b'a']),
            ("\0", &[0, 2, 0xc0, 0x80]),
            ("é", &[0, 2, 0xc3, 0xa9]),
            ("\u{d55c}", &[0, 3, 0xed, 0x95, 0x9c]),
            ("𝄞", &[0, 6, 0xed, 0xa0, 0xb4, 0xed, 0xb4, 0x9e]),
        ];
        for options in [WriteOptions::REFERENCE, WriteOptions::OPTIMISED] {
            for (s, want) in cases {
                let mut w = vec![0xaa];
                write_string(&mut w, s, options);
                assert_eq!(w[1..], *want, "{s:?} with {options:?}");
            }
        }
    }

    /// Heterogeneous list: written as a list of compounds (non-compounds and wrappers wrapped),
    /// read back unwrapped; expected bytes frozen.
    #[test]
    fn heterogeneous_list() {
        let wrapper = Nbt::Compound(vec![(String::new(), Nbt::Int(7))]);
        let v = Nbt::List(vec![Nbt::Int(1), Nbt::String("a".into()), Nbt::Compound(vec![]), wrapper.clone()]);
        let mut w = Vec::new();
        write(&mut w, Some(&v));
        #[rustfmt::skip]
        let want: Vec<u8> = vec![
            9, 10, 0, 0, 0, 4,
            3, 0, 0, 0, 0, 0, 1, 0,
            8, 0, 0, 0, 1, b'a', 0,
            0,
            10, 0, 0, 3, 0, 0, 0, 0, 0, 7, 0, 0,
        ];
        assert_eq!(w, want);
        assert_eq!(read(&mut Reader::new(&w)).unwrap(), Some(v));
        // Homogeneous list: common type, without wrapper; wrappers of a file unwrapped.
        let mut w = Vec::new();
        write(&mut w, Some(&Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)])));
        assert_eq!(w[1], 3);
        let file = [9u8, 10, 0, 0, 0, 1, 8, 0, 0, 0, 1, b'x', 0];
        assert_eq!(read(&mut Reader::new(&file)).unwrap(), Some(Nbt::List(vec![Nbt::String("x".into())])));
    }

    /// Typical chunk column: palettes, lists of compounds and of doubles, long arrays.
    pub(super) fn typical_chunk() -> Nbt {
        let state = |i: i32| {
            Nbt::Compound(vec![
                ("Name".into(), Nbt::String(format!("minecraft:block_{i}"))),
                ("Properties".into(), Nbt::Compound(vec![("facing".into(), Nbt::String("north".into()))])),
            ])
        };
        let section = |y: i32| {
            Nbt::Compound(vec![
                ("Y".into(), Nbt::Byte(y as i8)),
                (
                    "block_states".into(),
                    Nbt::Compound(vec![
                        ("palette".into(), Nbt::List((0..12).map(state).collect())),
                        ("data".into(), Nbt::LongArray(vec![0x1234_5678; 256])),
                    ]),
                ),
                (
                    "biomes".into(),
                    Nbt::Compound(vec![(
                        "palette".into(),
                        Nbt::List(vec![Nbt::String("minecraft:plains".into()), Nbt::String("minecraft:river".into())]),
                    )]),
                ),
            ])
        };
        let ent = |i: i32| {
            Nbt::Compound(vec![
                ("Pos".into(), Nbt::List(vec![Nbt::Double(f64::from(i)); 3])),
                ("Motion".into(), Nbt::List(vec![Nbt::Double(0.0); 3])),
                ("Rotation".into(), Nbt::List(vec![Nbt::Float(0.0); 2])),
            ])
        };
        Nbt::Compound(vec![
            ("sections".into(), Nbt::List((-4..20).map(section).collect())),
            ("entities".into(), Nbt::List((0..40).map(ent).collect())),
        ])
    }

    /// Bench (loaded machine): writing a typical chunk with detection of the lists' common type,
    /// against the old writing (type of the first element), then each writer option alone and
    /// together. `cargo test --release -p rc-proto -- --ignored list_write_bench --nocapture`.
    #[test]
    #[ignore]
    fn list_write_bench() {
        fn old_payload(w: &mut Vec<u8>, v: &Nbt) {
            match v {
                Nbt::List(items) => {
                    w.push(items.first().map_or(0, Nbt::type_id));
                    w.extend_from_slice(&(items.len() as i32).to_be_bytes());
                    for i in items {
                        old_payload(w, i);
                    }
                }
                Nbt::Compound(fields) => {
                    for (k, v) in fields {
                        w.push(v.type_id());
                        write_string(w, k, WriteOptions::REFERENCE);
                        old_payload(w, v);
                    }
                    w.push(0);
                }
                o => write_payload(w, o, WriteOptions::REFERENCE),
            }
        }
        let chunk = typical_chunk();
        let mut a = Vec::new();
        let mut b = Vec::new();
        old_payload(&mut a, &chunk);
        write_payload(&mut b, &chunk, WriteOptions::REFERENCE);
        assert_eq!(a, b, "same bytes on a homogeneous list");
        let n = 2000;
        let time = |w: &mut Vec<u8>, write: &dyn Fn(&mut Vec<u8>)| {
            let t = std::time::Instant::now();
            for _ in 0..n {
                w.clear();
                write(w);
            }
            t.elapsed() / n
        };
        let combos = [
            ("reference", WriteOptions::REFERENCE),
            ("string fast path", WriteOptions { string_fast_path: true, array_bulk: false }),
            ("array bulk", WriteOptions { string_fast_path: false, array_bulk: true }),
            ("both", WriteOptions::OPTIMISED),
        ];
        for pass in 0..3 {
            let old = time(&mut a, &|w| old_payload(w, &chunk));
            let mut line = format!("pass {pass}: first-element list type {old:?}");
            for (name, options) in combos {
                let t = time(&mut b, &|w| write_payload(w, &chunk, options));
                assert_eq!(a, b, "{name}: same bytes");
                line += &format!(", {name} {t:?}");
            }
            println!("{line} / chunk");
        }
    }

    /// String of more than 65,535 bytes: written empty, the rest of the stream stays readable.
    #[test]
    fn long_string_written_empty() {
        let v = Nbt::Compound(vec![("s".into(), Nbt::String("é".repeat(40_000))), ("n".into(), Nbt::Int(5))]);
        let mut w = Vec::new();
        write(&mut w, Some(&v));
        let back = read(&mut Reader::new(&w)).unwrap().unwrap();
        assert_eq!(back.get("s").and_then(Nbt::as_str), Some(""));
        assert_eq!(back.get("n").and_then(Nbt::as_i64), Some(5));
        for options in [WriteOptions::REFERENCE, WriteOptions::OPTIMISED] {
            let mut w = Vec::new();
            write_string(&mut w, &"a".repeat(65_535), options);
            assert_eq!(w.len(), 2 + 65_535);
        }
    }

    #[test]
    fn absent_and_hostile() {
        assert_eq!(read(&mut Reader::new(&[0])).unwrap(), None);
        // List announcing 2^31-1 elements: refused without allocation.
        let hostile = [9, 1, 0x7f, 0xff, 0xff, 0xff];
        assert!(read(&mut Reader::new(&hostile)).is_err());
        assert!(skip(&mut Reader::new(&hostile)).is_err());
        // Deep nesting.
        let mut deep = vec![9u8];
        for _ in 0..1000 {
            deep.extend_from_slice(&[9, 0, 0, 0, 1]);
        }
        assert!(read(&mut Reader::new(&deep)).is_err());
        assert!(read(&mut Reader::new(&[13])).is_err());
    }
}

#[cfg(test)]
#[path = "nbt_write_tests.rs"]
mod write_tests;
