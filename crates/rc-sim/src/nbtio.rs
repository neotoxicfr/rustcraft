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

/// Starting capacity of an encoded root when nothing better is known.
const DEFAULT_ROOT_CAPACITY: usize = 4096;

/// Empty named root, uncompressed.
pub fn encode_root(v: &Nbt) -> Vec<u8> {
    encode_root_with_capacity(v, DEFAULT_ROOT_CAPACITY)
}

fn encode_root_with_capacity(v: &Nbt, capacity: usize) -> Vec<u8> {
    let mut w = Vec::with_capacity(capacity);
    nbt::write_named(&mut w, "", v);
    w
}

/// True (default): [`RootEncoder`] sizes each buffer from the previous root.
/// `RUSTCRAFT_OPT_NBT_SIZE_HINT=0`: every root starts at 4 KiB.
fn size_hint_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_NBT_SIZE_HINT") != Some("0"))
}

/// Encodes a run of similar roots (the chunk columns of one save, the proto-chunks of one
/// batch). A column takes tens of KiB: starting at 4 KiB and doubling costs 3 to 5
/// reallocations and copies, so each buffer starts at the size of the previous root instead.
/// Under rayon's `map_init` there is one encoder per rayon task, not per thread: rayon splits a
/// batch into as many tasks as it likes, and the first root of each task starts at 4 KiB.
#[derive(Debug)]
pub struct RootEncoder {
    next_capacity: usize,
}

impl Default for RootEncoder {
    fn default() -> Self {
        RootEncoder { next_capacity: DEFAULT_ROOT_CAPACITY }
    }
}

impl RootEncoder {
    pub fn encode(&mut self, v: &Nbt) -> Vec<u8> {
        let bytes = encode_root_with_capacity(v, self.next_capacity);
        if size_hint_enabled() {
            self.next_capacity = capacity_after(bytes.len());
        }
        bytes
    }
}

/// Capacity for the root that follows one of `len` bytes: an eighth more, so that a slightly
/// bigger root still fits without a doubling; never below the default, so that small roots grow
/// no more often than before.
fn capacity_after(len: usize) -> usize {
    (len + len / 8).max(DEFAULT_ROOT_CAPACITY)
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

#[cfg(test)]
thread_local! {
    /// Tests: UUIDs formatted by [`uuid_text`] in this thread (proof that a team found by UUID
    /// formats none).
    pub(crate) static UUID_TEXTS_FORMATTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// UUID as text with dashes (player file names).
pub fn uuid_text(u: u128) -> String {
    #[cfg(test)]
    UUID_TEXTS_FORMATTED.with(|n| n.set(n.get() + 1));
    let h = format!("{u:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The UUID whose [`uuid_text`] is exactly `s` (32 lowercase hex digits grouped 8-4-4-4-12), read
/// without allocating; `None` for any other text (uppercase, short groups, signs…).
pub fn canonical_uuid(s: &str) -> Option<u128> {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    let mut u: u128 = 0;
    for (i, &c) in bytes.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if c != b'-' {
                return None;
            }
            continue;
        }
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => return None,
        };
        u = u << 4 | u128::from(digit);
    }
    Some(u)
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

    /// `canonical_uuid` = the command parser's UUID kept only when its text is canonical (the
    /// test of the former scoreboard paths), without allocating.
    #[test]
    fn canonical_uuid_is_the_inverse_of_uuid_text() {
        let former = |s: &str| crate::commands::snbt::parse_uuid(s).filter(|u| uuid_text(*u) == s);
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut samples = vec![0u128, u128::MAX, 1, 0x6c36f763_6526_3b48_9820_8f008743cbeb];
        samples.extend((0..1000).map(|_| u128::from(next()) << 64 | u128::from(next())));
        for u in samples {
            let text = uuid_text(u);
            assert_eq!(canonical_uuid(&text), Some(u), "{text}");
            for other in [text.to_uppercase(), text.replacen('-', "", 1), format!("{text}0"), text[1..].to_owned()] {
                assert_eq!(canonical_uuid(&other), former(&other), "{other}");
            }
        }
        for s in
            ["1-2-3-4-5", "", "Steve", "-0000001-0002-0003-0004-000000000005", "0000000g-0000-0000-0000-000000000000"]
        {
            assert_eq!(canonical_uuid(s), former(s), "{s}");
            assert_eq!(canonical_uuid(s), None, "{s}");
        }
    }

    /// Every chunk of the region files (terrain, entities, points of interest) of the test world,
    /// parsed; `None` without the test world.
    fn test_world_roots() -> Option<Vec<Nbt>> {
        let world = crate::tests::test_world()?;
        let mut roots = Vec::new();
        for entry in walk_files(&world) {
            let Some(name) = entry.file_name().and_then(|n| n.to_str()) else { continue };
            let mut parts = name.split('.');
            let (Some("r"), Some(rx), Some(rz), Some("mca")) = (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let (Ok(rx), Ok(rz)) = (rx.parse::<i32>(), rz.parse::<i32>()) else { continue };
            let mut region = crate::region::RegionFile::open(&entry, false).expect("region");
            let present: Vec<usize> = region.present().collect();
            for i in present {
                let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
                if let Ok(Some(bytes)) = region.read(i, cx, cz) {
                    roots.push(parse_root(&bytes).expect("chunk NBT"));
                }
            }
        }
        Some(roots)
    }

    fn walk_files(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() { files.extend(walk_files(&path)) } else { files.push(path) }
        }
        files
    }

    fn encode_with(v: &Nbt, capacity: usize, options: nbt::WriteOptions) -> Vec<u8> {
        let mut w = Vec::with_capacity(capacity);
        nbt::write_named_with(&mut w, "", v, options);
        w
    }

    /// Model world: every chunk encodes to the same bytes with the reference and the optimised
    /// writer, and the run encoder gives the bytes of `encode_root`.
    #[test]
    fn model_world_encodes_identically() {
        let Some(roots) = test_world_roots() else { return };
        assert!(roots.len() > 100, "{} chunks", roots.len());
        let mut encoder = RootEncoder::default();
        let mut bytes = 0;
        for (i, root) in roots.iter().enumerate() {
            let reference = encode_with(root, DEFAULT_ROOT_CAPACITY, nbt::WriteOptions::REFERENCE);
            assert_eq!(encode_with(root, 0, nbt::WriteOptions::OPTIMISED), reference, "chunk {i}");
            assert_eq!(encoder.encode(root), reference, "chunk {i}");
            bytes += reference.len();
        }
        eprintln!("{} chunks, {bytes} bytes: same encoding", roots.len());
    }

    /// Bench (loaded machine): encoding the real columns of the test world, reference writer
    /// with 4 KiB buffers against each option. `RUSTCRAFT_SOLO_WORLD=<world> cargo test --release
    /// -p rc-sim -- --ignored encode_root_bench --nocapture`.
    #[test]
    #[ignore = "bench"]
    fn encode_root_bench() {
        let roots = test_world_roots().expect("test world (RUSTCRAFT_SOLO_WORLD)");
        let columns: Vec<&Nbt> = roots.iter().filter(|r| r.get("sections").is_some()).collect();
        bench_columns("every column", &columns);
        // The columns an autosave writes: full ones, around the players.
        let full: Vec<&Nbt> = columns
            .iter()
            .copied()
            .filter(|r| r.get("Status").and_then(Nbt::as_str) == Some("minecraft:full"))
            .collect();
        bench_columns("full columns", &full);
    }

    fn bench_columns(label: &str, columns: &[&Nbt]) {
        let total: usize = columns.iter().map(|r| encode_root(r).len()).sum();
        eprintln!("{label}: {} columns, {} KiB on average", columns.len(), total / columns.len() / 1024);
        let time = |encode: &mut dyn FnMut(&Nbt) -> Vec<u8>| {
            let t = std::time::Instant::now();
            let mut sink = 0;
            for _ in 0..5 {
                for r in columns {
                    sink += encode(r).len();
                }
            }
            assert_eq!(sink, total * 5);
            t.elapsed() / (5 * columns.len() as u32)
        };
        use nbt::WriteOptions;
        let strings_only = WriteOptions { string_fast_path: true, array_bulk: false };
        let arrays_only = WriteOptions { string_fast_path: false, array_bulk: true };
        for pass in 0..3 {
            let reference = time(&mut |r| encode_with(r, DEFAULT_ROOT_CAPACITY, WriteOptions::REFERENCE));
            let strings = time(&mut |r| encode_with(r, DEFAULT_ROOT_CAPACITY, strings_only));
            let arrays = time(&mut |r| encode_with(r, DEFAULT_ROOT_CAPACITY, arrays_only));
            let both = time(&mut |r| encode_with(r, DEFAULT_ROOT_CAPACITY, WriteOptions::OPTIMISED));
            let mut hint = DEFAULT_ROOT_CAPACITY;
            let reference_with_hint = time(&mut |r| {
                let bytes = encode_with(r, hint, WriteOptions::REFERENCE);
                hint = capacity_after(bytes.len());
                bytes
            });
            let mut hint = DEFAULT_ROOT_CAPACITY;
            let all = time(&mut |r| {
                let bytes = encode_with(r, hint, WriteOptions::OPTIMISED);
                hint = capacity_after(bytes.len());
                bytes
            });
            eprintln!(
                "pass {pass}, per column: reference {reference:?}, strings {strings:?}, arrays {arrays:?}, \
                 both {both:?}, size hint alone {reference_with_hint:?}, all three {all:?}"
            );
        }
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
