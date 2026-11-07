//! Per-subsystem memory sampling of the integrated server, read by the `[perf] ram` line of the client
//! (`RUSTCRAFT_PERFLOG`). Without the variable: a boolean read once at startup, no walk,
//! no allocation.
//!
//! - Server: one pass over the chunks per report period (100 ticks), walked in bounded batches of
//!   `PER_TICK` chunks on the singleplayer thread (~11 ticks for 700 chunks), nothing between passes;
//!   totals published at the end of each pass. Bytes estimated from the real capacity of vectors and
//!   strings (heap only, allocator headers excluded).
//! - Generator: entries of its map per stage, biome cache, after each batch.
//! - Temporary counters to settle optimisation leads: writes of
//!   proto-chunks (total, distinct positions, became full), re-reads of protos.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use hashbrown::{HashMap, HashSet};
use rc_proto::nbt::Nbt;

/// Sampling active (`RUSTCRAFT_PERFLOG` present at startup).
pub fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| rc_data::env_flag!("RUSTCRAFT_PERFLOG"))
}

/// Items of the compound kept per chunk for its next save (`Chunk::root` and
/// `Chunk::disk_sections`), in the order of [`ROOT_PARTS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootPart {
    BlockData,
    BlockPalette,
    Biomes,
    Light,
    Heightmaps,
    BlockEntities,
    Ticks,
    Structures,
    Other,
}

/// Labels of the items of the kept compound (`[perf] ram`), indexed by [`RootPart`].
pub const ROOT_PARTS: [&str; 9] = [
    "sections.block_states.data",
    "sections.block_states.palette",
    "sections.biomes",
    "SkyLight/BlockLight",
    "Heightmaps",
    "block_entities",
    "ticks",
    "structures",
    "other",
];

/// Bytes per item of the kept compound, indexed by [`RootPart`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RootBytes(pub [u64; 9]);

impl std::ops::IndexMut<RootPart> for RootBytes {
    fn index_mut(&mut self, part: RootPart) -> &mut u64 {
        &mut self.0[part as usize]
    }
}

impl std::ops::Index<RootPart> for RootBytes {
    type Output = u64;
    fn index(&self, part: RootPart) -> &u64 {
        &self.0[part as usize]
    }
}

impl std::ops::AddAssign for RootBytes {
    fn add_assign(&mut self, other: RootBytes) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
    }
}

/// Kept compound of one chunk by item, its light layers shared with memory not counted.
#[cfg(test)]
pub(crate) fn kept_compound(c: &crate::chunk::Chunk) -> RootBytes {
    let mut arcs = HashMap::new();
    for l in c.sky.iter().chain(&c.block).flatten() {
        first_sight(&mut arcs, l);
    }
    let mut out = RootBytes::default();
    root_parts(&c.root, &mut out);
    disk_section_parts(c, &mut arcs, &mut out);
    out
}

/// Totals of a pass over the server's chunks (bytes unless noted).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Server {
    pub chunks: u64,
    /// Blocks and biomes decoded (`Container::heap_bytes`).
    pub sections: u64,
    /// Distinct light layers (a layer shared by several chunks counted once)
    /// and their count.
    pub light: u64,
    pub light_layers: u64,
    pub heightmaps: u64,
    pub block_entities: u64,
    /// Kept compound, per item of `ROOT_PARTS`.
    pub root: RootBytes,
    /// Frozen entities (NBT): count and bytes.
    pub entities: u64,
    pub entity_bytes: u64,
    /// Pending work: generator outputs not yet taken, batches of the write thread.
    pub gen_out: u64,
    pub writer_queue: u64,
}

/// Map of a generator after its last batch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gen {
    pub terrain: u64,
    pub decorated: u64,
    pub full: u64,
    pub disk_full: u64,
    /// Bytes of the kept blocks (`DecoChunk::blocks`).
    pub blocks: u64,
    pub biome_cache: u64,
    /// Forgotten chunks waiting for a write acknowledgement (`Worker::parked`) and bytes of their blocks.
    pub parked: u64,
    pub parked_bytes: u64,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
static GENS: Mutex<Option<HashMap<String, Gen>>> = Mutex::new(None);
static PROTO_WRITES: AtomicU64 = AtomicU64::new(0);
static PROTO_DISTINCT: AtomicU64 = AtomicU64::new(0);
static PROTO_TO_FULL: AtomicU64 = AtomicU64::new(0);
static PROTO_READS: AtomicU64 = AtomicU64::new(0);

/// Heap bytes of an NBT node and its children (real capacities).
pub fn nbt_heap(n: &Nbt) -> usize {
    match n {
        Nbt::ByteArray(v) => v.capacity(),
        Nbt::String(s) => s.capacity(),
        Nbt::IntArray(v) => v.capacity() * 4,
        Nbt::LongArray(v) => v.capacity() * 8,
        Nbt::List(v) => v.capacity() * size_of::<Nbt>() + v.iter().map(nbt_heap).sum::<usize>(),
        Nbt::Compound(v) => {
            v.capacity() * size_of::<(String, Nbt)>() + v.iter().map(|(k, c)| k.capacity() + nbt_heap(c)).sum::<usize>()
        }
        _ => 0,
    }
}

/// Splits the compound of a chunk by item of `ROOT_PARTS`.
pub fn root_parts(root: &Nbt, out: &mut RootBytes) {
    let Nbt::Compound(top) = root else { return };
    out[RootPart::Other] += (top.capacity() * size_of::<(String, Nbt)>()) as u64;
    for (k, v) in top {
        let part = match k.as_str() {
            "sections" => {
                let Nbt::List(secs) = v else {
                    out[RootPart::Other] += (k.capacity() + nbt_heap(v)) as u64;
                    continue;
                };
                out[RootPart::Other] += (k.capacity() + secs.capacity() * size_of::<Nbt>()) as u64;
                secs.iter().for_each(|s| section_parts(s, out));
                continue;
            }
            "Heightmaps" => RootPart::Heightmaps,
            "block_entities" => RootPart::BlockEntities,
            "block_ticks" | "fluid_ticks" | "PostProcessing" => RootPart::Ticks,
            "structures" => RootPart::Structures,
            _ => RootPart::Other,
        };
        out[part] += (k.capacity() + nbt_heap(v)) as u64;
    }
}

/// Splits a section compound by item of `ROOT_PARTS` (its own vector counted as `other`).
fn section_parts(s: &Nbt, out: &mut RootBytes) {
    let entry = size_of::<(String, Nbt)>();
    let Nbt::Compound(f) = s else { return };
    out[RootPart::Other] += (f.capacity() * entry) as u64;
    for (sk, sv) in f {
        let part = match (sk.as_str(), sv) {
            ("block_states", Nbt::Compound(b)) => {
                out[RootPart::Other] += (b.capacity() * entry + sk.capacity()) as u64;
                for (bk, bv) in b {
                    let part = if bk == "data" { RootPart::BlockData } else { RootPart::BlockPalette };
                    out[part] += (bk.capacity() + nbt_heap(bv)) as u64;
                }
                continue;
            }
            ("block_states", _) => RootPart::BlockPalette,
            ("biomes", _) => RootPart::Biomes,
            ("SkyLight" | "BlockLight", _) => RootPart::Light,
            _ => RootPart::Other,
        };
        out[part] += (sk.capacity() + nbt_heap(sv)) as u64;
    }
}

/// What a chunk keeps per height for its save (`Chunk::disk_sections`): the vector and boxes as
/// `other`, kept compounds by item, kept light sheets memory no longer holds (first sight in the
/// pass) as light.
fn disk_section_parts(c: &crate::chunk::Chunk, arcs: &mut HashMap<usize, Layer>, out: &mut RootBytes) {
    out[RootPart::Other] += (c.disk_sections.capacity() * size_of::<crate::chunk_disk::DiskSection>()) as u64;
    for d in &c.disk_sections {
        if let crate::chunk_disk::SectionForm::Kept(s) = &d.form {
            out[RootPart::Other] += size_of::<Nbt>() as u64;
            section_parts(s, out);
        }
        for kept in [&d.sky, &d.block] {
            if let crate::chunk_disk::KeptLight::Own(l) = kept
                && first_sight(arcs, l)
            {
                out[RootPart::Light] += rc_world::LIGHT_BYTES as u64;
            }
        }
    }
}

/// When a new pass over the chunks may start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassStart {
    /// Old path (`RUSTCRAFT_OPT_MEMWALK_ONCE_PER_REPORT=0`): a new pass right after the previous one,
    /// so ~11 passes per 5 s report on a 700-chunk world, 10 of them never read.
    Continuous,
    /// One pass every `TICKS_BETWEEN_PASSES` ticks (default), the first at the first tick.
    EveryReportPeriod,
    /// One pass when the `[perf] ram` report asked for one, the first at the first tick
    /// (`RUSTCRAFT_OPT_MEMWALK_ON_REPORT_REQUEST=1`, default 0).
    OnReportRequest,
}

/// Ticks between the starts of two passes: the `[perf] ram` line is written every 5 s, i.e. 100 ticks.
const TICKS_BETWEEN_PASSES: u32 = 100;

/// Pass start rule, read once at startup.
fn pass_start() -> PassStart {
    static ONCE_PER_REPORT: OnceLock<bool> = OnceLock::new();
    static ON_REQUEST: OnceLock<bool> = OnceLock::new();
    if !crate::generation::opt_switch(&ONCE_PER_REPORT, "RUSTCRAFT_OPT_MEMWALK_ONCE_PER_REPORT", true) {
        PassStart::Continuous
    } else if crate::generation::opt_switch(&ON_REQUEST, "RUSTCRAFT_OPT_MEMWALK_ON_REPORT_REQUEST", false) {
        PassStart::OnReportRequest
    } else {
        PassStart::EveryReportPeriod
    }
}

/// Set by `report` (client thread), consumed by the next pass start in `OnReportRequest` mode.
static REPORT_ASKED_FOR_A_PASS: AtomicBool = AtomicBool::new(false);

/// Pass in progress over the chunks of the singleplayer thread.
#[derive(Default)]
struct Walk {
    keys: Vec<(i32, i32)>,
    next: usize,
    acc: Server,
    /// Layers already seen in the pass, kept alive until its end: a layer freed during
    /// the pass cannot give its address to a new one, which would then be ignored.
    arcs: HashMap<usize, Layer>,
    /// Ticks left before the next pass may start (`EveryReportPeriod`); 0 = due now.
    ticks_until_next_pass: u32,
    passes_started: u64,
}

impl Walk {
    fn in_progress(&self) -> bool {
        self.next < self.keys.len()
    }

    /// Should a new pass start at this tick? Called once per tick, before the batch.
    fn pass_due(&mut self, start: PassStart, report_asked: &AtomicBool) -> bool {
        let countdown_over = self.ticks_until_next_pass == 0;
        self.ticks_until_next_pass = self.ticks_until_next_pass.saturating_sub(1);
        if self.in_progress() {
            return false;
        }
        let asked = report_asked.swap(false, Relaxed);
        // The first pass starts at once in every mode, so that the first report already has totals.
        self.passes_started == 0
            || match start {
                PassStart::Continuous => true,
                PassStart::EveryReportPeriod => countdown_over,
                PassStart::OnReportRequest => asked,
            }
    }

    fn begin_pass(&mut self, chunks: &HashMap<(i32, i32), Slot>) {
        self.arcs.clear();
        self.keys.clear();
        self.keys.extend(chunks.keys().copied());
        self.next = 0;
        self.passes_started += 1;
        self.ticks_until_next_pass = TICKS_BETWEEN_PASSES - 1;
    }

    /// Adds the next `PER_TICK` chunks of the pass to the totals.
    fn walk_batch(&mut self, chunks: &HashMap<(i32, i32), Slot>) {
        let end = (self.next + PER_TICK).min(self.keys.len());
        for &p in &self.keys[self.next..end] {
            let Some(Some(c)) = chunks.get(&p) else { continue };
            add_chunk(&mut self.acc, &mut self.arcs, c);
        }
        self.next = end;
    }

    /// Totals of the finished pass (none for an empty world, like the old path). The seen light
    /// layers are released at once: kept until the next pass, they would hold freed layers for 5 s.
    fn take_finished_pass(&mut self) -> Option<Server> {
        let walked_something = !self.keys.is_empty();
        self.arcs.clear();
        self.keys.clear();
        self.next = 0;
        let done = std::mem::take(&mut self.acc);
        walked_something.then_some(done)
    }

    /// One tick of sampling; gives the chunk totals of a pass to publish, if one is ready.
    /// `report_asked`: the report's request flag (a parameter so that tests use their own).
    fn tick(&mut self, chunks: &HashMap<(i32, i32), Slot>, start: PassStart, report_asked: &AtomicBool) -> Option<Server> {
        if start == PassStart::Continuous {
            return self.tick_continuous(chunks);
        }
        if self.pass_due(start, report_asked) {
            self.begin_pass(chunks);
        } else if !self.in_progress() {
            return None;
        }
        self.walk_batch(chunks);
        if self.in_progress() { None } else { self.take_finished_pass() }
    }

    /// Old path, kept as it was: a finished pass is published at the next tick, which starts the next pass.
    fn tick_continuous(&mut self, chunks: &HashMap<(i32, i32), Slot>) -> Option<Server> {
        let mut finished = None;
        if !self.in_progress() {
            if !self.keys.is_empty() {
                finished = Some(std::mem::take(&mut self.acc));
            }
            self.begin_pass(chunks);
        }
        self.walk_batch(chunks);
        finished
    }
}

type Slot = Option<Box<crate::chunk::Chunk>>;

/// Adds one chunk to the totals of the pass.
fn add_chunk(a: &mut Server, arcs: &mut HashMap<usize, Layer>, c: &crate::chunk::Chunk) {
    a.chunks += 1;
    a.sections += c.sections.iter().map(|s| s.blocks.heap_bytes() + s.biomes.heap_bytes()).sum::<usize>() as u64;
    for l in c.sky.iter().chain(&c.block).flatten() {
        if first_sight(arcs, l) {
            a.light += rc_world::LIGHT_BYTES as u64;
            a.light_layers += 1;
        }
    }
    a.heightmaps += c.heightmaps.iter().map(|h| h.1.len() * 8).sum::<usize>() as u64;
    a.block_entities += c.block_entities.iter().map(nbt_heap).sum::<usize>() as u64;
    root_parts(&c.root, &mut a.root);
    disk_section_parts(c, arcs, &mut a.root);
}

type Layer = std::sync::Arc<[u8; rc_world::LIGHT_BYTES]>;

/// Layer not yet seen in the pass? A clone is kept until the end of the pass.
fn first_sight(arcs: &mut HashMap<usize, Layer>, l: &Layer) -> bool {
    arcs.insert(std::sync::Arc::as_ptr(l) as usize, l.clone()).is_none()
}

thread_local! {
    static WALK: std::cell::RefCell<Walk> = std::cell::RefCell::new(Walk::default());
    static PROTOS: std::cell::RefCell<HashSet<(i32, i32)>> = std::cell::RefCell::new(HashSet::new());
}

/// Chunks sampled per tick (bound on the work added to the tick when sampling is active).
const PER_TICK: usize = 64;

impl crate::server::Server {
    /// One tick of the sampling pass (to call once per tick, sampling active only): a bounded batch
    /// of chunks while a pass runs, nothing between passes.
    pub(crate) fn mem_walk(&mut self) {
        let finished = WALK.with(|w| w.borrow_mut().tick(&self.chunks, pass_start(), &REPORT_ASKED_FOR_A_PASS));
        if let Some(chunk_totals) = finished {
            *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(self.with_pending_work(chunk_totals));
        }
    }

    /// Completes the chunk totals of a pass with the frozen entities and the pending work.
    fn with_pending_work(&self, mut done: Server) -> Server {
        done.entities = self.entities.values().map(|v| v.len() as u64).sum();
        done.entity_bytes = self
            .entities
            .values()
            .map(|v| (v.capacity() * size_of::<(i32, Nbt)>() + v.iter().map(|e| nbt_heap(&e.1)).sum::<usize>()) as u64)
            .sum();
        done.gen_out = self.generator.as_ref().map_or(0, |g| g.rx.len() as u64);
        done.writer_queue = self.writer.as_ref().map_or(0, |wr| wr.queued() as u64);
        done
    }
}

/// Generator `dim`: map after a batch.
pub(crate) fn publish_gen(dim: &str, g: Gen) {
    GENS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(dim.to_owned(), g);
}

/// Generator `dim` stopped: its map leaves the sampling.
pub(crate) fn forget_gen(dim: &str) {
    if let Some(m) = GENS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(dim);
    }
}

/// World opening: samplings and counters of the previous world cleared.
pub(crate) fn reset() {
    *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *GENS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    for c in [&PROTO_WRITES, &PROTO_DISTINCT, &PROTO_TO_FULL, &PROTO_READS] {
        c.store(0, Relaxed);
    }
    PROTOS.with(|s| *s.borrow_mut() = HashSet::new());
    WALK.with(|w| *w.borrow_mut() = Walk::default());
    REPORT_ASKED_FOR_A_PASS.store(false, Relaxed);
}

/// Proto positions tracked at most (≈ 1 MB): beyond that, new positions are no longer
/// tracked and "became full" becomes a lower bound, without making the sampling itself grow.
const PROTOS_MAX: usize = 1 << 16;

/// A proto-chunk written (singleplayer thread).
pub(crate) fn proto_written(p: (i32, i32)) {
    PROTO_WRITES.fetch_add(1, Relaxed);
    if PROTOS.with(|s| {
        let mut s = s.borrow_mut();
        s.len() < PROTOS_MAX && s.insert(p)
    }) {
        PROTO_DISTINCT.fetch_add(1, Relaxed);
    }
}

/// A full chunk arrived from the generator (singleplayer thread): had it gone through a written proto?
pub(crate) fn full_arrived(p: (i32, i32)) {
    if PROTOS.with(|s| s.borrow_mut().remove(&p)) {
        PROTO_TO_FULL.fetch_add(1, Relaxed);
    }
}

/// Protos re-read from disk by the generator.
pub(crate) fn protos_read(n: usize) {
    PROTO_READS.fetch_add(n as u64, Relaxed);
}

/// Latest samplings: server (last finished pass), generators per dimension, counters
/// (proto writes, distinct positions, became full, re-reads).
pub fn snapshot() -> (Option<Server>, Vec<(String, Gen)>, [u64; 4]) {
    let s = SERVER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut g: Vec<(String, Gen)> = GENS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    g.sort_by(|a, b| a.0.cmp(&b.0));
    let c = [PROTO_WRITES.load(Relaxed), PROTO_DISTINCT.load(Relaxed), PROTO_TO_FULL.load(Relaxed), PROTO_READS.load(Relaxed)];
    (s, g, c)
}

/// Readable sampling line (MB), for `[perf] ram`.
pub fn report() -> String {
    REPORT_ASKED_FOR_A_PASS.store(true, Relaxed);
    let (s, g, c) = snapshot();
    let mo = |b: u64| b as f64 / 1_048_576.0;
    let mut out = String::new();
    use std::fmt::Write;
    if let Some(s) = s {
        let root: u64 = s.root.0.iter().sum();
        let _ = write!(
            out,
            "server: {} chunks, sections {:.1} MB, light {:.1} MB ({} layers), heightmaps {:.1} MB, block entities {:.1} MB, kept compound {:.1} MB (",
            s.chunks,
            mo(s.sections),
            mo(s.light),
            s.light_layers,
            mo(s.heightmaps),
            mo(s.block_entities),
            mo(root)
        );
        for (i, k) in ROOT_PARTS.iter().enumerate() {
            let _ = write!(out, "{}{k} {:.1}", if i > 0 { ", " } else { "" }, mo(s.root.0[i]));
        }
        let _ = write!(
            out,
            "), frozen entities {} ({:.1} MB); pending: generator outputs {}, write batches {}",
            s.entities,
            mo(s.entity_bytes),
            s.gen_out,
            s.writer_queue
        );
    } else {
        out.push_str("server: pass in progress");
    }
    for (d, g) in g {
        let _ = write!(
            out,
            "; generator {d}: {} terrain, {} decorated, {} full (of which {} read), blocks {:.1} MB, biome cache {}, set aside {} ({:.1} MB)",
            g.terrain,
            g.decorated,
            g.full,
            g.disk_full,
            mo(g.blocks),
            g.biome_cache,
            g.parked,
            mo(g.parked_bytes)
        );
    }
    let _ = write!(out, "; protos written {} ({} positions, {} became full), re-read {}", c[0], c[1], c[2], c[3]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_parts_sum_to_the_whole_heap() {
        let sec = |y: i8| {
            Nbt::Compound(vec![
                ("Y".into(), Nbt::Byte(y)),
                (
                    "block_states".into(),
                    Nbt::Compound(vec![
                        ("palette".into(), Nbt::List(vec![Nbt::String("minecraft:stone".into())])),
                        ("data".into(), Nbt::LongArray(vec![0; 256])),
                    ]),
                ),
                ("biomes".into(), Nbt::Compound(vec![("palette".into(), Nbt::List(vec![]))])),
                ("SkyLight".into(), Nbt::ByteArray(vec![0; 2048])),
            ])
        };
        let root = Nbt::Compound(vec![
            ("sections".into(), Nbt::List(vec![sec(0), sec(1)])),
            ("Heightmaps".into(), Nbt::Compound(vec![("WORLD_SURFACE".into(), Nbt::LongArray(vec![0; 37]))])),
            ("block_entities".into(), Nbt::List(vec![])),
            ("Status".into(), Nbt::String("minecraft:full".into())),
        ]);
        let mut parts = RootBytes::default();
        root_parts(&root, &mut parts);
        assert_eq!(parts.0.iter().sum::<u64>(), nbt_heap(&root) as u64, "{parts:?}");
        assert!(parts[RootPart::BlockData] >= 2 * 256 * 8 && parts[RootPart::Light] >= 2 * 2048);
    }

    /// A seen layer stays alive during the pass: a layer freed then replaced during
    /// the pass cannot lend its address to the new one, which is therefore counted.
    #[test]
    fn light_dedup_holds_seen_layers() {
        let mut arcs = HashMap::new();
        let a: Layer = std::sync::Arc::new([1; rc_world::LIGHT_BYTES]);
        assert!(first_sight(&mut arcs, &a));
        assert!(!first_sight(&mut arcs, &a.clone()), "same layer counted twice");
        assert_eq!(std::sync::Arc::strong_count(&a), 2, "seen layer not kept");
        drop(a);
        for _ in 0..64 {
            let b: Layer = std::sync::Arc::new([2; rc_world::LIGHT_BYTES]);
            assert!(first_sight(&mut arcs, &b), "new layer mistaken for an already seen layer");
        }
        assert_eq!(arcs.len(), 65);
    }

    /// World opening: samplings of the previous one cleared; generator stopped: map forgotten;
    /// tracking of protos bounded.
    #[test]
    fn reset_forget_and_bounded_protos() {
        publish_gen("minecraft:the_nether", Gen { terrain: 3, ..Default::default() });
        publish_gen("minecraft:overworld", Gen { terrain: 5, ..Default::default() });
        forget_gen("minecraft:the_nether");
        assert_eq!(snapshot().1.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), ["minecraft:overworld"]);
        for i in 0..PROTOS_MAX as i32 + 100 {
            proto_written((i, 0));
        }
        assert_eq!(PROTOS.with(|s| s.borrow().len()), PROTOS_MAX);
        full_arrived((0, 0));
        protos_read(2);
        *SERVER.lock().unwrap() = Some(Server { chunks: 9, ..Default::default() });
        reset();
        let (s, g, c) = snapshot();
        assert!(s.is_none() && g.is_empty() && c == [0; 4], "{s:?} {g:?} {c:?}");
        assert_eq!(PROTOS.with(|s| s.borrow().len()), 0);
    }

    /// Kept compound of a chunk: `sections` sections of `longs` block longs each, both light
    /// arrays, height map, block entity, ticks and structures (every item of `ROOT_PARTS`).
    fn chunk_root(sections: i8, longs: usize) -> Nbt {
        let section = |y: i8| {
            Nbt::Compound(vec![
                ("Y".into(), Nbt::Byte(y)),
                (
                    "block_states".into(),
                    Nbt::Compound(vec![
                        ("palette".into(), Nbt::List((0..8).map(|i| Nbt::String(format!("minecraft:block_{i}"))).collect())),
                        ("data".into(), Nbt::LongArray(vec![0; longs])),
                    ]),
                ),
                ("biomes".into(), Nbt::Compound(vec![("palette".into(), Nbt::List(vec![Nbt::String("minecraft:plains".into())]))])),
                ("SkyLight".into(), Nbt::ByteArray(vec![0; 2048])),
                ("BlockLight".into(), Nbt::ByteArray(vec![0; 2048])),
            ])
        };
        Nbt::Compound(vec![
            ("sections".into(), Nbt::List((0..sections).map(section).collect())),
            ("Heightmaps".into(), Nbt::Compound(vec![("WORLD_SURFACE".into(), Nbt::LongArray(vec![0; 37]))])),
            ("block_entities".into(), Nbt::List(vec![Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:chest".into()))])])),
            ("block_ticks".into(), Nbt::List(vec![])),
            ("structures".into(), Nbt::Compound(vec![("starts".into(), Nbt::Compound(vec![]))])),
            ("Status".into(), Nbt::String("minecraft:full".into())),
        ])
    }

    /// `n` chunks (and a few known but absent positions) whose sizes vary with their index; every
    /// third chunk shares one sky layer, so that the light dedup is exercised across batches.
    fn frozen_chunks(n: i32, sections: i8, longs: usize) -> HashMap<(i32, i32), Slot> {
        use rc_world::{Container, Section};
        let shared: Layer = std::sync::Arc::new([7; rc_world::LIGHT_BYTES]);
        let mut map = HashMap::new();
        for i in 0..n {
            let packed = Container::Packed { bits: 4, palette: Some(vec![0; 16].into()), data: vec![0; 256].into() };
            let blocks = if i % 2 == 0 { packed } else { Container::Single(1) };
            let own_layer = || Some(std::sync::Arc::new([i as u8; rc_world::LIGHT_BYTES]));
            let chunk = crate::chunk::Chunk {
                x: i,
                z: -i,
                sections: vec![Section { non_empty: 1, blocks, biomes: Container::Single(0) }],
                sky: vec![if i % 3 == 0 { Some(shared.clone()) } else { own_layer() }, None, Some(rc_world::zero_layer())],
                block: vec![own_layer()],
                light_on: true,
                heightmaps: vec![(1, vec![0; 37].into())],
                block_entities: (0..i % 4).map(|_| Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:chest".into()))])).collect(),
                inhabited_time: 0,
                root: chunk_root(sections + (i % 3) as i8, longs),
                disk_sections: Vec::new(),
                lossy: false,
                changed: Vec::new(),
                biomes_changed: Vec::new(),
                dirty: false,
            };
            map.insert((i, -i), Some(Box::new(chunk)));
        }
        for i in 0..5 {
            map.insert((10_000 + i, 0), None);
        }
        map
    }

    /// Ticks the walk until it publishes; gives the totals and the number of ticks it took.
    fn ticks_until_published(walk: &mut Walk, chunks: &HashMap<(i32, i32), Slot>, start: PassStart) -> (Server, u32) {
        let asked = AtomicBool::new(false);
        for tick in 1..=1000 {
            if let Some(done) = walk.tick(chunks, start, &asked) {
                return (done, tick);
            }
        }
        panic!("no pass published in 1000 ticks ({start:?})");
    }

    /// Totals of the 300 frozen chunks, written down once from the old continuous pass.
    fn expected_totals() -> Server {
        Server {
            chunks: 300,
            sections: 150 * (16 * 4 + 256 * 8),
            // 300 block layers, 200 own sky layers, the shared sky layer and the zero layer.
            light: 502 * rc_world::LIGHT_BYTES as u64,
            light_layers: 502,
            heightmaps: 300 * 37 * 8,
            block_entities: EXPECTED_BLOCK_ENTITY_BYTES,
            root: RootBytes(EXPECTED_ROOT_PARTS),
            ..Default::default()
        }
    }
    const EXPECTED_BLOCK_ENTITY_BYTES: u64 = 32_850;
    const EXPECTED_ROOT_PARTS: [u64; 9] = [1_846_800, 467_100, 105_300, 3_702_600, 112_500, 35_700, 3_300, 21_600, 502_500];

    /// Same frozen world: the spaced pass publishes exactly the totals of the old continuous pass,
    /// and those totals are the stored ones (a pass published before its end would miss chunks).
    #[test]
    fn spaced_pass_publishes_the_same_totals_as_the_continuous_pass() {
        let chunks = frozen_chunks(300, 2, 256);
        let (old, old_ticks) = ticks_until_published(&mut Walk::default(), &chunks, PassStart::Continuous);
        let (new, new_ticks) = ticks_until_published(&mut Walk::default(), &chunks, PassStart::EveryReportPeriod);
        let (asked, _) = ticks_until_published(&mut Walk::default(), &chunks, PassStart::OnReportRequest);
        assert_eq!(new, old, "spaced pass differs from the continuous pass");
        assert_eq!(asked, old, "pass on request differs from the continuous pass");
        assert_eq!(old, expected_totals(), "totals of the frozen world changed");
        // 305 positions in batches of 64: 5 ticks; the old path publishes one tick later.
        assert_eq!((old_ticks, new_ticks), (6, 5));
    }

    /// 100 ticks = 1 pass started and published; the next starts at tick 101; between the two,
    /// no walk and no light layer kept alive.
    #[test]
    fn one_pass_per_hundred_ticks() {
        let chunks = frozen_chunks(300, 1, 16);
        let shared = chunks[&(0, 0)].as_ref().unwrap().sky[0].clone().unwrap();
        let held_by_the_world = std::sync::Arc::strong_count(&shared) - 1;
        let (mut walk, asked) = (Walk::default(), AtomicBool::new(false));
        let mut published = Vec::new();
        for tick in 1..=200 {
            if walk.tick(&chunks, PassStart::EveryReportPeriod, &asked).is_some() {
                published.push(tick);
                assert_eq!(std::sync::Arc::strong_count(&shared) - 1, held_by_the_world, "layers kept after the pass");
            }
            if tick == 100 {
                assert_eq!((walk.passes_started, published.len()), (1, 1), "100 ticks = one pass");
            }
        }
        assert_eq!(published, [5, 105]);
        assert_eq!(walk.passes_started, 2);
    }

    /// On request: the first pass at once, then nothing until the report asks, then one pass.
    #[test]
    fn pass_on_request_waits_for_the_report() {
        let chunks = frozen_chunks(100, 1, 16);
        let (mut walk, asked) = (Walk::default(), AtomicBool::new(false));
        let first: Vec<u32> =
            (1..=300).filter(|_| walk.tick(&chunks, PassStart::OnReportRequest, &asked).is_some()).collect();
        assert_eq!((first.as_slice(), walk.passes_started), ([2].as_slice(), 1));
        asked.store(true, Relaxed);
        let second: Vec<(u32, u64)> = (1..=300)
            .filter_map(|tick| walk.tick(&chunks, PassStart::OnReportRequest, &asked).map(|s| (tick, s.chunks)))
            .collect();
        assert_eq!((second.as_slice(), walk.passes_started), ([(2, 100)].as_slice(), 2));
        assert!(!asked.load(Relaxed), "request not consumed");
    }

    /// Cost of one full pass (all batches) over a bench-sized world: 700 chunks of 24 sections.
    /// `cargo test --release -p rc-sim -- --ignored --nocapture memory_walk_pass_cost`
    #[test]
    #[ignore]
    fn memory_walk_pass_cost() {
        let chunks = frozen_chunks(700, 24, 342);
        let asked = AtomicBool::new(false);
        for start in [PassStart::Continuous, PassStart::EveryReportPeriod] {
            let mut walk = Walk::default();
            let mut per_tick = Vec::new();
            let t = std::time::Instant::now();
            for _ in 0..1000 {
                let tick = std::time::Instant::now();
                std::hint::black_box(walk.tick(&chunks, start, &asked));
                per_tick.push(tick.elapsed().as_secs_f64() * 1e3);
            }
            let total = t.elapsed().as_secs_f64() * 1e3;
            per_tick.sort_by(f64::total_cmp);
            eprintln!(
                "{start:?}: {:.3} ms per 100 ticks, worst tick {:.3} ms, {} passes",
                total / 10.0,
                per_tick[per_tick.len() - 1],
                walk.passes_started
            );
        }
    }
}
