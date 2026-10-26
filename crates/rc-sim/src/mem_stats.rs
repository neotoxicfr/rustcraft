//! Per-subsystem memory sampling of the integrated server, read by the `[perf] ram` line of the client
//! (`RUSTCRAFT_PERFLOG`). Without the variable: a boolean read once at startup, no walk,
//! no allocation.
//!
//! - Server: chunks walked in bounded batches at every tick on the singleplayer thread, the pass
//!   resuming at the counter; totals published at the end of each pass. Bytes estimated from the
//!   real capacity of vectors and strings (heap only, allocator headers excluded).
//! - Generator: entries of its map per stage, biome cache, after each batch.
//! - Temporary counters to settle optimisation leads: writes of
//!   proto-chunks (total, distinct positions, became full), re-reads of protos.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

use hashbrown::{HashMap, HashSet};
use rc_proto::nbt::Nbt;

/// Sampling active (`RUSTCRAFT_PERFLOG` present at startup).
pub fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| rc_data::env_flag!("RUSTCRAFT_PERFLOG"))
}

/// Items of the compound kept per chunk (`Chunk::root`, rewritten at each save).
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
    pub root: [u64; 9],
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
pub fn root_parts(root: &Nbt, out: &mut [u64; 9]) {
    let Nbt::Compound(top) = root else { return };
    let entry = size_of::<(String, Nbt)>();
    out[8] += (top.capacity() * size_of::<(String, Nbt)>()) as u64;
    for (k, v) in top {
        let i = match k.as_str() {
            "sections" => {
                let Nbt::List(secs) = v else {
                    out[8] += (k.capacity() + nbt_heap(v)) as u64;
                    continue;
                };
                out[8] += (k.capacity() + secs.capacity() * size_of::<Nbt>()) as u64;
                for s in secs {
                    let Nbt::Compound(f) = s else { continue };
                    out[8] += (f.capacity() * entry) as u64;
                    for (sk, sv) in f {
                        let part = match sk.as_str() {
                            "block_states" => {
                                if let Nbt::Compound(b) = sv {
                                    out[8] += (b.capacity() * entry) as u64;
                                    for (bk, bv) in b {
                                        let j = if bk == "data" { 0 } else { 1 };
                                        out[j] += (bk.capacity() + nbt_heap(bv)) as u64;
                                    }
                                    out[8] += sk.capacity() as u64;
                                    continue;
                                }
                                1
                            }
                            "biomes" => 2,
                            "SkyLight" | "BlockLight" => 3,
                            _ => 8,
                        };
                        out[part] += (sk.capacity() + nbt_heap(sv)) as u64;
                    }
                }
                continue;
            }
            "Heightmaps" => 4,
            "block_entities" => 5,
            "block_ticks" | "fluid_ticks" | "PostProcessing" => 6,
            "structures" => 7,
            _ => 8,
        };
        out[i] += (k.capacity() + nbt_heap(v)) as u64;
    }
}

/// Pass in progress over the chunks of the singleplayer thread.
#[derive(Default)]
struct Walk {
    keys: Vec<(i32, i32)>,
    next: usize,
    acc: Server,
    /// Layers already seen in the pass, kept alive until its end: a layer freed during
    /// the pass cannot give its address to a new one, which would then be ignored.
    arcs: HashMap<usize, Layer>,
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
    /// One batch of the sampling pass (to call once per tick, sampling active only).
    pub(crate) fn mem_walk(&mut self) {
        WALK.with(|w| {
            let mut w = w.borrow_mut();
            if w.next >= w.keys.len() {
                if !w.keys.is_empty() {
                    let mut done = std::mem::take(&mut w.acc);
                    done.entities = self.entities.values().map(|v| v.len() as u64).sum();
                    done.entity_bytes = self
                        .entities
                        .values()
                        .map(|v| (v.capacity() * size_of::<(i32, Nbt)>() + v.iter().map(|e| nbt_heap(&e.1)).sum::<usize>()) as u64)
                        .sum();
                    done.gen_out = self.generator.as_ref().map_or(0, |g| g.rx.len() as u64);
                    done.writer_queue = self.writer.as_ref().map_or(0, |wr| wr.queued() as u64);
                    *SERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(done);
                }
                w.arcs.clear();
                w.keys.clear();
                w.keys.extend(self.chunks.keys().copied());
                w.next = 0;
            }
            let end = (w.next + PER_TICK).min(w.keys.len());
            for i in w.next..end {
                let p = w.keys[i];
                let Some(Some(c)) = self.chunks.get(&p) else { continue };
                let w = &mut *w;
                let a = &mut w.acc;
                a.chunks += 1;
                a.sections += c.sections.iter().map(|s| s.blocks.heap_bytes() + s.biomes.heap_bytes()).sum::<usize>() as u64;
                for l in c.sky.iter().chain(&c.block).flatten() {
                    if first_sight(&mut w.arcs, l) {
                        a.light += rc_world::LIGHT_BYTES as u64;
                        a.light_layers += 1;
                    }
                }
                a.heightmaps += c.heightmaps.iter().map(|h| h.1.len() * 8).sum::<usize>() as u64;
                a.block_entities += c.block_entities.iter().map(nbt_heap).sum::<usize>() as u64;
                root_parts(&c.root, &mut a.root);
            }
            w.next = end;
        });
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
    let (s, g, c) = snapshot();
    let mo = |b: u64| b as f64 / 1_048_576.0;
    let mut out = String::new();
    use std::fmt::Write;
    if let Some(s) = s {
        let root: u64 = s.root.iter().sum();
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
            let _ = write!(out, "{}{k} {:.1}", if i > 0 { ", " } else { "" }, mo(s.root[i]));
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
        let mut parts = [0u64; 9];
        root_parts(&root, &mut parts);
        assert_eq!(parts.iter().sum::<u64>(), nbt_heap(&root) as u64, "{parts:?}");
        assert!(parts[0] >= 2 * 256 * 8 && parts[3] >= 2 * 2048);
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
}
