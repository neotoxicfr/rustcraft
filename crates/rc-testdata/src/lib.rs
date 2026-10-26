//! Real out-of-repository data for tests and benches: 26.3 version cache and reference
//! capture of the official client. Everything returns `None` if they are absent: the tests
//! depending on them are then skipped, except under `RUSTCRAFT_OPT_REQUIRE=1` (integration bench,
//! equivalence proofs) where missing data is an explicit failure.

use std::path::PathBuf;

use rc_data::{Bound, Phase, VersionData};
use rc_proto::Reader;
use rc_proto::varint::{peek_varint, read_varint};

pub const VERSION: &str = "26.3";

/// `RUSTCRAFT_OPT_REQUIRE=1`: out-of-repository data is required.
pub fn required() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_REQUIRE") == Some("1")
}

/// `None` → test ignored (message), or failure if the data is required.
pub fn need<T>(v: Option<T>, what: &str) -> Option<T> {
    if v.is_none() {
        assert!(!required(), "missing data: {what} (RUSTCRAFT_OPT_REQUIRE=1)");
        eprintln!("missing data: {what}, test skipped");
    }
    v
}

/// Version data from the local cache (without preparation).
pub fn version_data() -> Option<VersionData> {
    need(rc_data::load_cached(VERSION), "26.3 data cache")
}

/// A recorded packet (format `docs/specs/capture.md`).
pub struct Record<'a> {
    pub to_server: bool,
    /// 0 handshake, 1 status, 2 login, 3 configuration, 4 play.
    pub phase: u8,
    /// VarInt identifier + body, decompressed.
    pub packet: &'a [u8],
}

/// The biggest capture available (the game session, not a simple ping).
pub fn reference_capture() -> Option<Vec<u8>> {
    need(find_capture(), "reference capture")
}

fn find_capture() -> Option<Vec<u8>> {
    let dir = PathBuf::from(rc_data::env_os!("LOCALAPPDATA")?).join("rustcraft-re").join("captures");
    let best = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "rccap"))
        .max_by_key(|e| e.metadata().map(|m| m.len()).unwrap_or(0))?;
    std::fs::read(best.path()).ok()
}

pub fn records(bytes: &[u8]) -> Vec<Record<'_>> {
    assert_eq!(&bytes[..8], b"RCCAP\x00\x00\x01", "capture header");
    let mut out = Vec::new();
    let mut p = 8;
    while p < bytes.len() {
        let (to_server, phase) = (bytes[p + 8] == 1, bytes[p + 9]);
        let (len, n) = peek_varint(&bytes[p + 10..]).expect("length").expect("complete length");
        let start = p + 10 + n;
        out.push(Record { to_server, phase, packet: &bytes[start..start + len as usize] });
        p = start + len as usize;
    }
    out
}

/// Body (after the identifier) of the server → client play packets named `name`.
pub fn play_packets(cap: &[u8], data: &VersionData, name: &str) -> Vec<Vec<u8>> {
    let Some(id) = data.packet_id(Phase::Play, Bound::ToClient, name) else { return Vec::new() };
    records(cap)
        .into_iter()
        .filter(|r| !r.to_server && r.phase == 4)
        .filter_map(|r| {
            let mut rd = Reader::new(r.packet);
            (read_varint(&mut rd).ok()? == id).then(|| rd.rest().to_vec())
        })
        .collect()
}
