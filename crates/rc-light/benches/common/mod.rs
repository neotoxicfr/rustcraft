// Direct read allowed (tests and benches: settings read at launch, off the measured path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Common tools of the light benches: grid as a table (like the tests' grid), reading
//! of the reference capture outside the repository, reading and replay of the relight traces written by
//! the integrated server (`RUSTCRAFT_LIGHTTRACE`), fingerprints and counting allocator.
//!
//! Format of a trace (little-endian), written by `rc-sim/src/play.rs`:
//! - header: `RCLTRACE`, u32 version (1), u32 height, u8 sky, u8 width of the global
//!   palette, u16 length + name of the dimension;
//! - then records, one tag byte:
//!   - 1, column seen for the first time: i32 cx, i32 cz, u16 sections, 4,096 u32 states per
//!     section, then `sections + 2` sky sheets and as many block sheets (u8 present + 2,048 bytes);
//!   - 2, relight: u64 tick, u32 n, then n × (i32 x, i32 y from the bottom of the world, i32 z,
//!     u32 placed state). The columns it can read are written before it.
#![allow(dead_code)]

use hashbrown::HashMap;
use std::sync::Arc;

use rc_light::{LightGrid, Touched};
use rc_world::{Container, LIGHT_BYTES, LightLayer, Section};

pub type Col = (Vec<Section>, Vec<LightLayer>, Vec<LightLayer>);

#[derive(Clone, Default)]
pub struct MapGrid(pub HashMap<(i32, i32), Col>);

impl LightGrid for MapGrid {
    fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])> {
        self.0.get(&(cx, cz)).map(|(s, k, b)| (&s[..], &k[..], &b[..]))
    }
    fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]> {
        self.0.get_mut(&(cx, cz)).map(|(_, k, b)| if sky { &mut k[..] } else { &mut b[..] })
    }
}

pub fn required() -> bool {
    std::env::var("RUSTCRAFT_OPT_REQUIRE").is_ok_and(|v| v == "1")
}

/// `None` → bench skipped, or explicit failure with `RUSTCRAFT_OPT_REQUIRE=1`.
pub fn need<T>(v: Option<T>, what: &str) -> Option<T> {
    if v.is_none() {
        assert!(!required(), "{what} absent (RUSTCRAFT_OPT_REQUIRE=1)");
        eprintln!("{what} absent: bench skipped");
    }
    v
}

/// Expected result of data recognised by its fingerprint; unknown data is only
/// checked against the frozen reference (failure with `RUSTCRAFT_OPT_REQUIRE=1`).
pub fn check_known(what: &str, input: u64, output: u64, known: &[(u64, u64)]) {
    match known.iter().find(|k| k.0 == input) {
        Some(&(_, want)) => {
            assert_eq!(output, want, "{what}: output fingerprint {output:016x}, expected {want:016x}");
            eprintln!("{what}: known data {input:016x}, expected output {output:016x}");
        }
        None => {
            assert!(
                !required(),
                "{what}: unknown data {input:016x} (output {output:016x}) with RUSTCRAFT_OPT_REQUIRE=1"
            );
            eprintln!(
                "{what}: unknown data {input:016x}, output {output:016x} (to record), only the reference checked"
            );
        }
    }
}

pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x100_0000_01b3);
        }
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

/// Fingerprint of the sheets of a grid (sorted columns) and of the touched sections.
pub fn digest(grid: &MapGrid, touched: &[Touched]) -> u64 {
    let mut h = Fnv::default();
    let mut keys: Vec<_> = grid.0.keys().copied().collect();
    keys.sort_unstable();
    for k in keys {
        let (_, sky, block) = &grid.0[&k];
        h.u64(k.0 as u64);
        h.u64(k.1 as u64);
        for l in sky.iter().chain(block) {
            match l {
                Some(a) => h.bytes(&a[..]),
                None => h.u64(0xdead),
            }
        }
    }
    for t in touched {
        let mut v: Vec<_> = t.iter().map(|(&k, &b)| (k, b)).collect();
        v.sort_unstable();
        for ((x, s, z), b) in v {
            h.u64(x as u64);
            h.u64(s as u64);
            h.u64(z as u64);
            h.u64(u64::from(b));
        }
    }
    h.0
}

/// Absent sheets filled like the server before a relight: sky = lowest plane of the
/// first present sheet above (otherwise 15), block = zero.
pub fn fill_implied(sky: &mut [LightLayer], block: &mut [LightLayer]) {
    for i in (0..sky.len()).rev() {
        fill_one(sky, block, i);
    }
    for i in 0..block.len() {
        fill_one(sky, block, i);
    }
}

/// Sheet `i` alone (sky and block), same rule as `fill_implied`.
fn fill_one(sky: &mut [LightLayer], block: &mut [LightLayer], i: usize) {
    if sky.get(i).is_some_and(Option::is_none) {
        let mut out = [0xffu8; LIGHT_BYTES];
        if let Some(above) = sky[i + 1..].iter().flatten().next() {
            for y in 0..16 {
                out[y * 128..y * 128 + 128].copy_from_slice(&above[..128]);
            }
        }
        sky[i] = Some(Arc::new(out));
    }
    if block.get(i).is_some_and(Option::is_none) {
        block[i] = Some(Arc::new([0u8; LIGHT_BYTES]));
    }
}

/// What the server does before a relight: absent sheets filled in the 3 × 3 columns
/// and the 3 sheets around each change (`rc-sim/src/play.rs`, `flush_block_changes`).
pub fn fill_around(g: &mut MapGrid, p: [i32; 3]) {
    for dx in -1..=1 {
        for dz in -1..=1 {
            let Some((_, sky, block)) = g.0.get_mut(&((p[0] >> 4) + dx, (p[2] >> 4) + dz)) else { continue };
            let si = (p[1] >> 4) + 1;
            for i in [si - 1, si, si + 1] {
                if let Ok(i) = usize::try_from(i) {
                    fill_one(sky, block, i);
                }
            }
        }
    }
}

/// Columns of the reference capture (including the official game's light) and its fingerprint.
pub struct Capture {
    pub digest: u64,
    pub height: i32,
    pub direct_bits: u8,
    pub cols: HashMap<(i32, i32), Col>,
}

pub fn capture(d: &rc_data::VersionData) -> Option<Capture> {
    use rc_proto::Reader;
    use rc_proto_v777::chunk::{ChunkContext, read_chunk};
    let cap = rc_testdata::reference_capture()?;
    let mut h = Fnv::default();
    h.bytes(&cap);
    let ctx = ChunkContext::new(24, d.states.len() as u32, 67);
    let mut cols = HashMap::new();
    for p in rc_testdata::play_packets(&cap, d, "minecraft:level_chunk_with_light") {
        let c = read_chunk(&mut Reader::new(&p), &ctx).ok()?;
        cols.insert((c.x, c.z), (c.sections, c.light.sky, c.light.block));
    }
    Some(Capture { digest: h.0, height: 24 * 16, direct_bits: d.state_bits() as u8, cols })
}

/// States of a flat column, index `(y << 8) | (z << 4) | x` (input of `light_chunk`).
pub fn flat_states(sections: &[Section]) -> Vec<u32> {
    let mut out = vec![0u32; sections.len() * 4096];
    for (i, s) in sections.iter().enumerate() {
        s.blocks.unpack_into(&mut out[i * 4096..(i + 1) * 4096]);
    }
    out
}

/// One relight: positions (y from the bottom of the world) and placed states.
pub type Event = Vec<([i32; 3], u32)>;

pub enum Step {
    Column((i32, i32), Box<Col>),
    Event(Event),
}

pub struct Trace {
    pub name: String,
    pub digest: u64,
    pub height: i32,
    pub has_sky: bool,
    pub direct_bits: u8,
    pub steps: Vec<Step>,
}

impl Trace {
    pub fn events(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s, Step::Event(_))).count()
    }
    pub fn changes(&self) -> usize {
        self.steps.iter().map(|s| if let Step::Event(e) = s { e.len() } else { 0 }).sum()
    }
}

struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(self.u32()? as i32)
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn layers(&mut self, n: usize) -> Option<Vec<LightLayer>> {
        (0..n)
            .map(|_| match self.u8()? {
                0 => Some(None),
                _ => Some(Some(Arc::new(<[u8; LIGHT_BYTES]>::try_from(self.take(LIGHT_BYTES)?).ok()?))),
            })
            .collect()
    }
}

/// Reads a trace; `None` if the header or a record is unreadable (truncated trace:
/// the complete records before the cut are kept).
pub fn read_trace(name: &str, bytes: &[u8]) -> Option<Trace> {
    let mut h = Fnv::default();
    h.bytes(bytes);
    let mut r = Rd(bytes);
    if r.take(8)? != b"RCLTRACE" || r.u32()? != 1 {
        return None;
    }
    let height = r.u32()? as i32;
    let has_sky = r.u8()? != 0;
    let direct_bits = r.u8()?;
    let n = r.u16()? as usize;
    r.take(n)?;
    let mut steps = Vec::new();
    while let Some(tag) = r.u8() {
        let step = (|| match tag {
            1 => {
                let (cx, cz) = (r.i32()?, r.i32()?);
                let n = r.u16()? as usize;
                let mut sections = Vec::with_capacity(n);
                let mut values = [0u32; 4096];
                for _ in 0..n {
                    for v in values.iter_mut() {
                        *v = r.u32()?;
                    }
                    let non_empty = values.iter().filter(|&&v| v != 0).count() as i16;
                    let mut s = Section::empty(0, 0);
                    s.blocks = Container::pack(&values, 4, 8, direct_bits);
                    s.non_empty = non_empty;
                    sections.push(s);
                }
                let sky = r.layers(n + 2)?;
                let block = r.layers(n + 2)?;
                Some(Step::Column((cx, cz), Box::new((sections, sky, block))))
            }
            2 => {
                let _tick = r.u64()?;
                let n = r.u32()? as usize;
                let e = (0..n).map(|_| Some(([r.i32()?, r.i32()?, r.i32()?], r.u32()?))).collect::<Option<Event>>()?;
                Some(Step::Event(e))
            }
            _ => None,
        })();
        match step {
            Some(s) => steps.push(s),
            None => break,
        }
    }
    Some(Trace { name: name.to_string(), digest: h.0, height, has_sky, direct_bits, steps })
}

/// Traces present: `RUSTCRAFT_LIGHTTRACE_DIR`, otherwise `%LOCALAPPDATA%\rustcraft-re\traces\lumiere`
/// (`.rclt` files, sorted by name).
pub fn traces() -> Vec<Trace> {
    let dir = std::env::var_os("RUSTCRAFT_LIGHTTRACE_DIR").map(std::path::PathBuf::from).or_else(|| {
        Some(
            std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
                .join("rustcraft-re")
                .join("traces")
                .join("lumiere"),
        )
    });
    let Some(Ok(rd)) = dir.map(std::fs::read_dir) else { return Vec::new() };
    let mut paths: Vec<_> =
        rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "rclt")).collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|p| {
            let name = p.file_stem()?.to_string_lossy().into_owned();
            let t = read_trace(&name, &std::fs::read(p).ok()?);
            if t.is_none() {
                eprintln!("trace {name} unreadable, ignored");
            }
            t
        })
        .collect()
}

/// Allocations made in the relight function alone during the replays (`memstats`).
pub static IN_ALLOCS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static IN_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Sets the states of a relight and fills the absent sheets around (like the server); returns
/// the changed positions.
pub fn apply_event(g: &mut MapGrid, tr: &Trace, e: &Event) -> Vec<[i32; 3]> {
    let mut pos = Vec::with_capacity(e.len());
    for &(p, state) in e {
        if let Some((sections, _, _)) = g.0.get_mut(&(p[0] >> 4, p[2] >> 4))
            && let Some(sec) = sections.get_mut((p[1] >> 4) as usize)
        {
            let i = rc_world::block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize);
            sec.set_blocks(&[(i as u16, state)], tr.direct_bits, |s| s == 0);
        }
        pos.push(p);
    }
    for &p in &pos {
        fill_around(g, p);
    }
    pos
}

/// Replays a trace with the relight function `f`: returns the final grid, the touched
/// sections of each relight and the time spent in `f` only.
pub fn replay(
    tr: &Trace,
    t: &rc_light::LightTables,
    f: impl Fn(&mut MapGrid, i32, &rc_light::LightTables, bool, &[[i32; 3]]) -> Touched,
) -> (MapGrid, Vec<Touched>, std::time::Duration) {
    use std::sync::atomic::Ordering::Relaxed;
    let mut g = MapGrid::default();
    let mut touched = Vec::new();
    let mut spent = std::time::Duration::ZERO;
    for s in &tr.steps {
        match s {
            Step::Column(k, c) => {
                g.0.entry(*k).or_insert_with(|| (**c).clone());
            }
            Step::Event(e) => {
                let pos = apply_event(&mut g, tr, e);
                let (a, b) = (ALLOCS.load(Relaxed), BYTES.load(Relaxed));
                let t0 = std::time::Instant::now();
                touched.push(f(&mut g, tr.height, t, tr.has_sky, &pos));
                spent += t0.elapsed();
                IN_ALLOCS.fetch_add(ALLOCS.load(Relaxed) - a, Relaxed);
                IN_BYTES.fetch_add(BYTES.load(Relaxed) - b, Relaxed);
            }
        }
    }
    (g, touched, spent)
}

pub static ALLOCS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Counting allocator (`--features memstats`): number and bytes of allocations, around
/// mimalloc. Without this option, mimalloc alone and no cost.
#[cfg(feature = "memstats")]
#[allow(unsafe_code)]
mod counting {
    use super::{ALLOCS, BYTES};
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::Ordering::Relaxed;

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(l.size() as u64, Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: `p` comes from this allocator with the same layout.
            unsafe { mimalloc::MiMalloc.dealloc(p, l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(l.size() as u64, Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.alloc_zeroed(l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(n as u64, Relaxed);
            // SAFETY: `p` comes from this allocator with layout `l`.
            unsafe { mimalloc::MiMalloc.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static GLOBAL: Counting = Counting;
}

#[cfg(not(feature = "memstats"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// (allocations, bytes) during `f` (zeros without `memstats`).
pub fn allocs<R>(f: impl FnOnce() -> R) -> (R, u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    let (a, b) = (ALLOCS.load(Relaxed), BYTES.load(Relaxed));
    let r = f();
    (r, ALLOCS.load(Relaxed) - a, BYTES.load(Relaxed) - b)
}
