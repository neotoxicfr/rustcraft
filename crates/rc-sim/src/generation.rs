//! Generation of missing chunks (`generation-decoration.md` § 1) on a dedicated thread, without blocking the
//! tick: statuses in the official order (terrain → 3×3 decoration → light → full).
//!
//! The thread owns the proto-chunks (status `terrain` or decorated): those on disk (written by the
//! game or by us) are reread, the others generated. To make a chunk C full: terrain of the
//! 5×5 around C, decoration of the 3×3 (each one's decoration writes into its neighbours), light
//! of C over its 3×3 area, then conversion. Modified proto-chunks go to the server as NBT
//! (written to disk the way the game does: when evicted from the table and at save, or at
//! every batch with `RUSTCRAFT_OPT_PROTO_LAZY=0`); a full chunk is returned to it to be sent and
//! saved. Parallel decoration by classes (cx mod 3, cz mod 3): two 3×3 regions of the same
//! class are disjoint, so the result only depends on the order of the classes.

use hashbrown::{HashMap, HashSet};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use rayon::prelude::*;
use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use rc_world::LightLayer;

/// Redone light of a full chunk: (position, sky, blocks).
type Relit = ((i32, i32), Vec<LightLayer>, Vec<LightLayer>);
use rc_worldgen::chunkgen::{DimGen, FlatSpec, GenSpec, SourceSpec, WorldData};
use rc_worldgen::decor_region::DecoChunk;
use rc_worldgen::terrain::BiomeCache;

use crate::chunk::{Chunk, Height};
use crate::generation_nbt::{Conv, DECORATED, RANK_FEATURES, RANK_FULL, RANK_TERRAIN, TERRAIN};
use crate::nbtio;
use crate::states::States;
use crate::world::Dimension;

/// Chunks targeted per batch (close to one another: the server sorts them by distance).
const BATCH: usize = 24;
/// Proto-chunks kept in the table at most (≈ 400 KB each); the evicted ones whose write
/// is not yet acknowledged stay apart (`Parked`).
const KEEP: usize = 900;

#[cfg(test)]
thread_local! {
    /// Tests: cap of the generator map started from this thread (evictions exercised).
    pub(crate) static TEST_KEEP: std::cell::Cell<usize> = const { std::cell::Cell::new(KEEP) };
    /// Tests: lazy write of proto-chunks imposed on the generator started from this thread.
    pub(crate) static TEST_PROTO_LAZY: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    /// Tests: flush without waiting imposed on the saves made from this thread.
    pub(crate) static TEST_PROTO_FLUSH_ASYNC: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// `RUSTCRAFT_OPT_PROTO_LAZY` (default 1; `0`: all modified proto-chunks written at every
/// batch): a proto-chunk is only written when evicted from the table or at save; a proto
/// that became full in the meantime never is (the full one replaces it).
fn proto_lazy() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = opt_switch(&C, "RUSTCRAFT_OPT_PROTO_LAZY", true);
    #[cfg(test)]
    let on = TEST_PROTO_LAZY.with(std::cell::Cell::get).unwrap_or(on);
    on
}

/// `RUSTCRAFT_OPT_PROTO_FLUSH_ASYNC` (default 1; `0`: awaited): the automatic save and
/// the pause one request the pending proto-chunks without waiting for them (written right after
/// `level.dat`, at the next tick's pickup); closing always waits for them. Waiting would make
/// the server thread wait until the end of the current batch (even the creation of the generator),
/// which writing at every batch never did.
fn proto_flush_async() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let on = opt_switch(&C, "RUSTCRAFT_OPT_PROTO_FLUSH_ASYNC", true);
    #[cfg(test)]
    let on = TEST_PROTO_FLUSH_ASYNC.with(std::cell::Cell::get).unwrap_or(on);
    on
}

/// Switch `RUSTCRAFT_OPT_<name>` read once: `1` enables, `0` disables, otherwise `default`.
pub(crate) fn opt_switch(cell: &'static std::sync::OnceLock<bool>, name: &str, default: bool) -> bool {
    *cell.get_or_init(|| match rc_data::env_flags::read_os(name).as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("0") => false,
        Some("1") => true,
        _ => default,
    })
}

/// Fallback of the stronghold rings: all computed when the generator is created, before the first batch
/// (`RUSTCRAFT_OPT_RINGS_WARM`, default 1: bench on a quiet machine, crowd generation −20 %, flight
/// p99 −10 %; `=0`: lazy per stronghold).
fn rings_warm() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_RINGS_WARM", true)
}

/// Stronghold rings computed in full as soon as the generator has nothing left to do after its
/// first batch (/locate without waiting; `RUSTCRAFT_OPT_RINGS_BG`, default 0 until measured).
fn rings_bg() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_RINGS_BG", false)
}

/// Generator of a dimension as the world settings write it
/// (`data/minecraft/world_gen_settings.dat`), and the seed.
pub fn read_spec(world: &Path, dimension: &str) -> Option<(GenSpec, i64)> {
    let root = nbtio::read_dat(&world.join("data").join("minecraft").join("world_gen_settings.dat")).ok()?;
    let data = root.get("data")?;
    let seed = data.get("seed")?.as_i64()?;
    let g = data.get("dimensions")?.get(dimension)?.get("generator")?;
    Some((spec_from_nbt(g)?, seed))
}

/// `generate_structures` setting of the world.
pub fn structures_enabled(world: &Path) -> bool {
    nbtio::read_dat(&world.join("data").join("minecraft").join("world_gen_settings.dat"))
        .ok()
        .and_then(|r| r.get("data")?.get("generate_structures")?.as_i64())
        .is_some_and(|v| v != 0)
}

/// Settings of a generator (`generator` compound).
pub fn spec_from_nbt(g: &Nbt) -> Option<GenSpec> {
    match g.get("type")?.as_str()? {
        "minecraft:noise" => {
            let settings = g.get("settings")?.as_str()?.to_owned();
            let bs = g.get("biome_source")?;
            let source = match bs.get("type")?.as_str()? {
                "minecraft:multi_noise" => SourceSpec::MultiNoise(bs.get("preset")?.as_str()?.to_owned()),
                "minecraft:the_end" => SourceSpec::TheEnd,
                "minecraft:fixed" => SourceSpec::Fixed(bs.get("biome")?.as_str()?.to_owned()),
                "minecraft:checkerboard" => SourceSpec::Checkerboard {
                    biomes: match bs.get("biomes")? {
                        Nbt::String(s) => vec![s.clone()],
                        Nbt::List(l) => l.iter().filter_map(Nbt::as_str).map(str::to_owned).collect(),
                        _ => return None,
                    },
                    scale: nbtio::int(bs, "scale").map_or(2, |v| v as i32),
                },
                _ => return None,
            };
            Some(GenSpec::Noise { settings, source })
        }
        "minecraft:flat" => {
            let s = g.get("settings")?;
            let layers = nbtio::list(s.get("layers"))
                .iter()
                .filter_map(|l| Some((l.get("block")?.as_str()?.to_owned(), nbtio::int(l, "height")? as i32)))
                .collect();
            let flag = |k: &str| nbtio::int(s, k).is_some_and(|v| v != 0);
            Some(GenSpec::Flat(FlatSpec {
                layers,
                biome: s.get("biome").and_then(Nbt::as_str).unwrap_or("minecraft:plains").to_owned(),
                features: flag("features"),
                lakes: flag("lakes"),
                // List of ids, or a single id / `#tag`; absent: all.
                structure_overrides: match s.get("structure_overrides") {
                    Some(Nbt::String(v)) => Some(vec![v.clone()]),
                    Some(Nbt::List(l)) => Some(l.iter().filter_map(Nbt::as_str).map(str::to_owned).collect()),
                    _ => None,
                },
            }))
        }
        "minecraft:debug" => Some(GenSpec::Debug),
        _ => None,
    }
}

/// `reports/` folder of the official data generator prepared with the version data
/// (biome parameter lists).
pub fn reports_dir(data: &VersionData) -> PathBuf {
    rc_data::cache_root().join(&data.version).join("generated").join("reports")
}

enum Msg {
    Want(Vec<(i32, i32)>),
    /// Save: proto-chunks not yet written are returned (followed by an `Out::Batch`), then an
    /// end signal on the given channel.
    Flush(Sender<()>),
}

/// What the generation thread returns to the server.
pub(crate) enum Out {
    /// Full chunk, to send and save, with the entities created by its structures.
    Full(Box<Chunk>, Vec<Nbt>),
    /// Redone light of an already returned full chunk (neighbour that arrived since).
    Relight((i32, i32), Vec<LightLayer>, Vec<LightLayer>),
    /// Modified proto-chunk: uncompressed NBT, to write into its region.
    Proto((i32, i32), Vec<u8>),
    /// Generation impossible (data missing): nothing more will be returned.
    Disabled(String),
    /// End of the numbered batch: all its outputs precede this message.
    Batch(u64),
}

/// Server side of the generation thread.
pub(crate) struct Gen {
    tx: Option<Sender<Msg>>,
    pub(crate) rx: Receiver<Out>,
    /// Chunks requested and not yet returned.
    pub(crate) pending: HashSet<(i32, i32)>,
    thread: Option<JoinHandle<()>>,
    /// World seed (decoration seed of the animals placed at generation).
    pub(crate) seed: i64,
    /// Animals placed at chunk generation: noise generator whose settings do not
    /// forbid it (the End forbids it; flat and debug never place any).
    pub(crate) mob_gen: bool,
    /// Last batch whose outputs have all been received (`Out::Batch`).
    pub(crate) seen: u64,
    /// Last batch acknowledged to the write thread for the proto-chunks.
    acked: u64,
    /// Acknowledgements returned to the thread: proto-chunks written / full chunks saved up to this batch.
    pub(crate) written: Arc<AtomicU64>,
    pub(crate) saved: Arc<AtomicU64>,
    /// Loading-screen map (first world opened): statuses set by the thread.
    pub(crate) prep: Arc<std::sync::OnceLock<Arc<crate::prep::Prep>>>,
    /// Proto-chunks written only on eviction and at save (`RUSTCRAFT_OPT_PROTO_LAZY`).
    lazy: bool,
}

/// What the thread needs to generate a dimension.
pub(crate) struct Setup {
    pub data: Arc<VersionData>,
    pub world: PathBuf,
    pub dimension: String,
    pub height: Height,
    pub biome_registry: Vec<String>,
    pub spec: GenSpec,
    pub seed: i64,
}

impl Gen {
    pub(crate) fn start(s: Setup) -> Option<Gen> {
        let seed = s.seed;
        let mob_gen =
            matches!(&s.spec, GenSpec::Noise { settings, .. } if settings.trim_start_matches("minecraft:") != "end");
        let (tx, rx_in) = crossbeam_channel::unbounded::<Msg>();
        let (tx_out, rx) = crossbeam_channel::unbounded::<Out>();
        #[cfg(test)]
        let keep = TEST_KEEP.with(std::cell::Cell::get);
        #[cfg(not(test))]
        let keep = KEEP;
        let lazy = proto_lazy();
        let (written, saved) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let acks = (written.clone(), saved.clone());
        let prep: Arc<std::sync::OnceLock<Arc<crate::prep::Prep>>> = Arc::default();
        let pr = prep.clone();
        let thread = std::thread::Builder::new()
            .name("singleplayer-generation".into())
            .spawn(move || match Worker::new(s, tx_out.clone()) {
                Ok(mut w) => {
                    w.keep = keep;
                    w.prep = pr;
                    w.lazy = lazy;
                    (w.written, w.saved) = acks;
                    w.run(rx_in)
                }
                Err(e) => {
                    let _ = tx_out.send(Out::Disabled(e));
                }
            })
            .ok()?;
        Some(Gen {
            tx: Some(tx),
            rx,
            pending: HashSet::new(),
            thread: Some(thread),
            seed,
            mob_gen,
            seen: 0,
            acked: 0,
            written,
            saved,
            prep,
            lazy,
        })
    }

    /// Asks the thread for its proto-chunks not yet written (lazy write only); returns
    /// the end-signal channel (outputs then all in `rx`).
    fn flush(&self) -> Option<Receiver<()>> {
        let tx = self.tx.as_ref().filter(|_| self.lazy)?;
        let (done, wait) = crossbeam_channel::bounded(1);
        tx.send(Msg::Flush(done)).ok()?;
        Some(wait)
    }

    /// Request for full chunks (in priority order).
    pub(crate) fn want(&mut self, list: Vec<(i32, i32)>) {
        let list: Vec<(i32, i32)> = list.into_iter().filter(|p| self.pending.insert(*p)).collect();
        if list.is_empty() {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Want(list));
        }
    }

    /// Stop: the current batch finishes, its outputs stay readable in `rx`.
    pub(crate) fn stop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Terrain,
    Decorated,
    Full,
}

struct Slot {
    c: Box<DecoChunk>,
    stage: Stage,
    /// Full chunk read from disk: read-only neighbour, never rewritten.
    disk_full: bool,
    dirty: bool,
    /// Block entities read from disk with the chunk.
    bes: Vec<Nbt>,
    /// Batch of its last send to the server (proto-chunk written or full chunk), 0: never.
    sent: u64,
}

impl Slot {
    /// Proto-chunk modified since its last send (to write before evicting it).
    fn unwritten(&self) -> bool {
        self.dirty && self.stage < Stage::Full && !self.disk_full
    }
}

/// Chunk evicted from the table whose latest version may not be on disk yet
/// (the server writes it on another thread, later): reread from here, never from disk, until
/// the acknowledgement has covered it.
enum Parked {
    /// Proto-chunk sent: kept whole until the write thread's acknowledgement.
    Proto(Slot),
    /// Full chunk returned to the server (written only at save): it only serves as a
    /// read neighbour (light, post-processing), only its blocks are kept, as runs.
    Full { runs: Vec<(u32, u32)>, min_y: i32, height: i32, sent: u64 },
}

/// Runs (state, length) of a block array.
fn pack(blocks: &[u32]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &b in blocks {
        match runs.last_mut() {
            Some((s, n)) if *s == b => *n += 1,
            _ => runs.push((b, 1)),
        }
    }
    runs.shrink_to_fit();
    runs
}

fn unpack(runs: &[(u32, u32)], out: &mut Vec<u32>) {
    out.clear();
    for &(s, n) in runs {
        out.extend(std::iter::repeat_n(s, n as usize));
    }
}

pub(crate) struct Worker {
    data: Arc<VersionData>,
    states: States,
    dg: DimGen,
    /// Generation data shared (`WorldData::shared`): held as long as the generator lives,
    /// so that `/locate`, `/place` and the other dimensions reuse them without rereading the jar.
    _wd: Arc<WorldData>,
    light: rc_light::LightTables,
    has_sky: bool,
    dim: Dimension,
    height: Height,
    map: HashMap<(i32, i32), Slot>,
    biomes: BiomeCache,
    out: Sender<Out>,
    order: DecorOrder,
    /// Feature types encountered that are not reproduced (name → count).
    missing: std::collections::BTreeMap<String, u32>,
    /// Structure starts read from a "structure_starts" proto-chunk on disk (map
    /// references included), carried over as is when this chunk is generated.
    saved_structures: HashMap<(i32, i32), Nbt>,
    /// Cap of chunks kept in memory (`KEEP`; lowered by the tests).
    keep: usize,
    /// Name of the dimension (memory pickup).
    dim_name: String,
    /// Evicted chunks not yet covered by an acknowledgement (reread from here before the disk).
    parked: HashMap<(i32, i32), Parked>,
    /// Full chunks returned by this thread since it started, never removed (≈ 20 bytes each): reread
    /// from disk, they stay ours (relit) and count as full for eviction.
    made: HashSet<(i32, i32)>,
    /// Number of the current batch (1 for the first).
    seq: u64,
    /// Server acknowledgements: proto-chunks written / full chunks saved up to the given batch
    /// (never advanced without a server: generation without a thread, new world).
    written: Arc<AtomicU64>,
    saved: Arc<AtomicU64>,
    /// Loading-screen map, set by the server (empty: nothing to mark).
    prep: Arc<std::sync::OnceLock<Arc<crate::prep::Prep>>>,
    /// Proto-chunks written only on eviction and at flush (`RUSTCRAFT_OPT_PROTO_LAZY`).
    lazy: bool,
}

/// Decoration order of a batch (the result at the borders depends on it, as in the game).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecorOrder {
    /// Classes (cx mod 3, cz mod 3) increasing, regions of a class in parallel.
    #[default]
    Classes,
    ClassesReversed,
    /// One chunk at a time, list in reverse.
    Sequential,
}

impl Worker {
    pub(crate) fn new(s: Setup, out: Sender<Out>) -> Result<Worker, String> {
        let t = std::time::Instant::now();
        let jar = crate::registries::client_jar(&s.data).ok_or("game jar not found")?;
        let reports = reports_dir(&s.data);
        let wd = WorldData::shared(&jar, reports.is_dir().then_some(reports.as_path()), &s.data)
            .map_err(|e| format!("generation data: {e}"))?;
        let mut dg = DimGen::new(&wd, &s.spec, &s.dimension, s.seed).map_err(|e| format!("generator: {e}"))?;
        if structures_enabled(&s.world) {
            dg.enable_structures(&wd).map_err(|e| format!("structures : {e}"))?;
            if rings_warm() {
                dg.warm_rings();
            }
        }
        let has_sky = s.data.dimension_types.iter().find(|t| t.name == s.dimension).is_none_or(|t| t.has_skylight);
        eprintln!("singleplayer: generator {} ready in {:.0} ms", s.dimension, t.elapsed().as_secs_f64() * 1e3);
        Ok(Worker {
            states: States::new(&s.data, s.biome_registry),
            light: rc_light::LightTables::new(&s.data),
            dim: Dimension::new(&s.world, &s.dimension, s.height),
            height: s.height,
            data: s.data,
            dg,
            _wd: wd,
            has_sky,
            map: HashMap::new(),
            biomes: BiomeCache::default(),
            out,
            order: DecorOrder::Classes,
            missing: Default::default(),
            saved_structures: HashMap::new(),
            keep: KEEP,
            dim_name: s.dimension,
            parked: HashMap::new(),
            made: HashSet::new(),
            seq: 0,
            written: Arc::default(),
            saved: Arc::default(),
            prep: Arc::default(),
            lazy: proto_lazy(),
        })
    }

    /// Status reached by chunks of the batch, on the loading-screen map.
    fn mark(&self, list: &[(i32, i32)], s: crate::prep::Status) {
        if let Some(p) = self.prep.get() {
            for &q in list {
                p.raise(q, s);
            }
        }
    }

    /// Message received: requests queued, flush done on the spot (between two batches).
    fn take(&mut self, m: Msg, queue: &mut VecDeque<(i32, i32)>) {
        match m {
            Msg::Want(l) => queue.extend(l),
            Msg::Flush(done) => {
                self.flush();
                let _ = done.send(());
            }
        }
    }

    fn run(&mut self, rx: Receiver<Msg>) {
        let mut queue: VecDeque<(i32, i32)> = VecDeque::new();
        // Full rings to compute at the first idle time after a batch.
        let mut rings_pending = rings_bg();
        let mut batches = 0usize;
        loop {
            if queue.is_empty() && rings_pending && batches > 0 && rx.is_empty() {
                rings_pending = false;
                let t = std::time::Instant::now();
                self.dg.warm_rings();
                if rc_data::env_flag!("RUSTCRAFT_GENLOG") {
                    eprintln!("singleplayer: stronghold rings computed in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
                }
            }
            // Blocking wait only without work.
            if queue.is_empty() {
                match rx.recv() {
                    Ok(m) => self.take(m, &mut queue),
                    // Closing (queue finished): remaining proto-chunks returned before the save.
                    Err(_) => {
                        self.flush();
                        return;
                    }
                }
            }
            while let Ok(m) = rx.try_recv() {
                self.take(m, &mut queue);
            }
            if queue.is_empty() {
                continue;
            }
            let batch: Vec<(i32, i32)> = queue.drain(..queue.len().min(BATCH)).collect();
            let t = std::time::Instant::now();
            let n = self.process(&batch);
            batches += 1;
            if rc_data::env_flag!("RUSTCRAFT_GENLOG") {
                eprintln!("singleplayer: generated {n} chunks in {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            }
        }
    }

    /// Chunk kept in memory (full as long as a neighbour is not).
    pub(crate) fn chunk(&self, p: (i32, i32)) -> Option<&DecoChunk> {
        self.map.get(&p).map(|s| &*s.c)
    }

    pub(crate) fn dim_gen(&self) -> &DimGen {
        &self.dg
    }

    pub(crate) fn states(&self) -> &States {
        &self.states
    }

    fn stage(&self, p: (i32, i32)) -> Option<Stage> {
        self.map.get(&p).map(|s| s.stage)
    }

    fn conv(&self) -> Conv<'_> {
        Conv::new(&self.data, &self.states, &self.dg.blocks, self.height, &self.dg.biome_names)
    }

    /// Makes the requested chunks full; returns their number.
    pub(crate) fn process(&mut self, batch: &[(i32, i32)]) -> usize {
        // A full chunk evicted but not yet saved stays full (never redone).
        self.restore(batch);
        let targets: Vec<(i32, i32)> =
            batch.iter().copied().filter(|&p| self.stage(p) != Some(Stage::Full) || self.map[&p].disk_full).collect();
        if targets.is_empty() {
            return 0;
        }
        self.seq += 1;
        let around = |list: &[(i32, i32)]| -> Vec<(i32, i32)> {
            let mut seen = HashSet::new();
            let mut v = Vec::new();
            for &(x, z) in list {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if seen.insert((x + dx, z + dz)) {
                            v.push((x + dx, z + dz));
                        }
                    }
                }
            }
            v
        };
        // (1) Neighbours of the targets, then neighbours of those to decorate: reread from disk if they are there.
        // A full chunk evicted without an overlay (`evict`) only has full chunks within two chunks:
        // it is never in these lists nor a neighbour of a relight.
        let near = around(&targets);
        self.load(&near);
        let decorate: Vec<(i32, i32)> =
            near.into_iter().filter(|&p| self.stage(p).is_none_or(|s| s == Stage::Terrain)).collect();
        let terrain: Vec<(i32, i32)> = around(&decorate);
        self.load(&terrain);
        // (2) Terrain of the missing ones, in parallel.
        let missing: Vec<(i32, i32)> = terrain.into_iter().filter(|p| !self.map.contains_key(p)).collect();
        if !missing.is_empty() {
            self.dg.prepare_biomes(&mut self.biomes, &missing);
            let g = &self.dg;
            let cache = &self.biomes;
            let saved = &self.saved_structures;
            let made: Vec<DecoChunk> = missing
                .par_iter()
                .map(|&(x, z)| {
                    let mut c = g.terrain(x, z, cache, None);
                    c.structures = Some(saved.get(&(x, z)).cloned().unwrap_or_else(|| g.structures_nbt(x, z)));
                    c
                })
                .collect();
            for c in &made {
                self.saved_structures.remove(&(c.cx, c.cz));
            }
            for c in made {
                self.map.insert(
                    (c.cx, c.cz),
                    Slot {
                        c: Box::new(c),
                        stage: Stage::Terrain,
                        disk_full: false,
                        dirty: true,
                        bes: Vec::new(),
                        sent: 0,
                    },
                );
            }
            if self.biomes.chunks.len() > 20_000 {
                self.biomes = BiomeCache::default();
            }
            self.mark(&missing, crate::prep::Status::Terrain);
        }
        // (3) Decoration by disjoint classes.
        let todo: Vec<(i32, i32)> =
            decorate.iter().copied().filter(|&p| self.stage(p) == Some(Stage::Terrain)).collect();
        self.decorate(&todo);
        self.mark(&todo, crate::prep::Status::Features);
        // (4) Post-processing, light and conversion of the targets.
        for p in &targets {
            let Some(s) = self.map.get_mut(p) else { continue };
            if s.disk_full {
                continue;
            }
            let mut c = std::mem::replace(&mut s.c, placeholder());
            {
                let map = &self.map;
                let b = &self.dg.blocks;
                let outside = |q: [i32; 3]| -> u32 {
                    match map.get(&(q[0] >> 4, q[2] >> 4)) {
                        Some(o) if !o.c.blocks.is_empty() => o.c.blocks[o.c.index(q[0], q[1], q[2])],
                        _ => b.default_state(b.k.air),
                    }
                };
                // Marked mushrooms: their survival depends on light (lit chunk first).
                let k = &b.k;
                let mushrooms = c.post_process.iter().any(|q| {
                    let s = c.blocks[c.index(q[0], q[1], q[2])];
                    b.is(s, k.brown_mushroom) || b.is(s, k.red_mushroom)
                });
                if mushrooms {
                    let (x, z) = *p;
                    let cols: rc_light::Columns = std::array::from_fn(|i| {
                        let q = (x + i as i32 % 3 - 1, z + i as i32 / 3 - 1);
                        if q == *p { Some(&c.blocks[..]) } else { map.get(&q).map(|s| &s.c.blocks[..]) }
                    });
                    let (sky, block) =
                        rc_light::light_chunk(&self.light, cols, self.height.sections as i32 * 16, self.has_sky);
                    let min_y = c.min_y;
                    let bright = |q: [i32; 3]| -> u8 {
                        let ly = q[1] - min_y;
                        let li = (ly >> 4) as usize + 1;
                        let i = rc_world::block_index((q[0] & 15) as usize, (ly & 15) as usize, (q[2] & 15) as usize);
                        let nib = |l: &rc_world::LightLayer| l.as_ref().map_or(0, |a| (a[i / 2] >> ((i & 1) * 4)) & 15);
                        nib(&sky[li]).max(nib(&block[li]))
                    };
                    rc_worldgen::decor_region::post_process_full_lit(&mut c, b, &outside, Some(&bright));
                } else {
                    rc_worldgen::decor_region::post_process_full(&mut c, b, &outside);
                }
            }
            c.post_process.clear();
            if let Some(s) = self.map.get_mut(p) {
                s.c = c;
            }
        }
        let conv = self.conv();
        let map = &self.map;
        let light = &self.light;
        let height = self.height.sections as i32 * 16;
        let has_sky = self.has_sky;
        // Scans of the columns read by the batch's lighting, done once per chunk (and not
        // by each of the 9 lightings that read it): the blocks no longer change until the end of the
        // batch (post-processing finished; `load` only adds missing ones).
        let mut scans: HashMap<(i32, i32), rc_light::ColumnScan> = HashMap::new();
        let cache = full_cache();
        if cache {
            scan_missing(&mut scans, map, light, height, &around(&targets));
        }
        let fulls: Vec<(Chunk, Vec<Nbt>)> = targets
            .par_iter()
            .filter_map(|&(x, z)| {
                let cols: rc_light::Columns = std::array::from_fn(|i| {
                    map.get(&(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1)).map(|s| &s.c.blocks[..])
                });
                let (sky, block) = light_scanned(light, cols, &scans, (x, z), height, has_sky);
                let s = map.get(&(x, z))?;
                Some((conv.full_chunk(&s.c, sky, block, 0, &s.bes), s.c.entities.clone()))
            })
            .collect();
        // Neighbours already full (generated here): light redone with the newcomers.
        let mut relit: Vec<(i32, i32)> = Vec::new();
        for &(x, z) in &targets {
            for i in 0..9 {
                let q = (x + i % 3 - 1, z + i / 3 - 1);
                if !targets.contains(&q)
                    && !relit.contains(&q)
                    && map.get(&q).is_some_and(|s| s.stage == Stage::Full && !s.disk_full)
                {
                    relit.push(q);
                }
            }
        }
        drop(conv);
        // Their evicted neighbours count as if they had stayed: overlay, then disk.
        self.load(&around(&relit));
        let (map, light) = (&self.map, &self.light);
        if cache {
            scan_missing(&mut scans, map, light, height, &around(&relit));
        }
        let relights: Vec<Relit> = relit
            .par_iter()
            .map(|&(x, z)| {
                let cols: rc_light::Columns = std::array::from_fn(|i| {
                    map.get(&(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1)).map(|s| &s.c.blocks[..])
                });
                let (sky, block) = light_scanned(light, cols, &scans, (x, z), height, has_sky);
                ((x, z), sky, block)
            })
            .collect();
        if rc_data::env_flag!("RUSTCRAFT_GENLOG") {
            eprintln!(
                "singleplayer: light of {} new chunks, {} relit neighbours, {} columns scanned",
                targets.len(),
                relit.len(),
                scans.len()
            );
        }
        let n = fulls.len();
        self.mark(&targets, crate::prep::Status::Light);
        for (c, ents) in fulls {
            if let Some(s) = self.map.get_mut(&(c.x, c.z)) {
                s.stage = Stage::Full;
                s.disk_full = false;
                s.dirty = false;
                s.sent = self.seq;
                // The entities leave with the full chunk (entity storage).
                s.c.entities.clear();
            }
            self.made.insert((c.x, c.z));
            let _ = self.out.send(Out::Full(Box::new(c), ents));
        }
        for (p, sky, block) in relights {
            let _ = self.out.send(Out::Relight(p, sky, block));
        }
        // (5) Modified proto-chunks to disk: at every batch (old path); otherwise on
        // eviction (`evict`) and at save (`flush`) only.
        if !self.lazy {
            let dirty = self.unwritten();
            self.send_protos(&dirty);
        }
        self.evict(targets[0]);
        if crate::mem_stats::enabled() {
            let mut g = crate::mem_stats::Gen { biome_cache: self.biomes.chunks.len() as u64, ..Default::default() };
            for s in self.map.values() {
                *match s.stage {
                    Stage::Terrain => &mut g.terrain,
                    Stage::Decorated => &mut g.decorated,
                    Stage::Full => &mut g.full,
                } += 1;
                g.disk_full += u64::from(s.disk_full);
                g.blocks += (s.c.blocks.capacity() * size_of::<u32>()) as u64;
            }
            g.parked = self.parked.len() as u64;
            for p in self.parked.values() {
                g.parked_bytes += match p {
                    Parked::Proto(s) => s.c.blocks.capacity() * size_of::<u32>(),
                    Parked::Full { runs, .. } => runs.capacity() * size_of::<(u32, u32)>(),
                } as u64;
            }
            crate::mem_stats::publish_gen(&self.dim_name, g);
        }
        let _ = self.out.send(Out::Batch(self.seq));
        n
    }

    /// Proto-chunks of the table modified since their last send (never a full one).
    fn unwritten(&self) -> Vec<(i32, i32)> {
        self.map.iter().filter(|(_, s)| s.unwritten()).map(|(p, _)| *p).collect()
    }

    /// Encodes (in parallel) and sends to the server the proto-chunks `list`, marked sent at the
    /// current batch (their write acknowledgement follows this batch's `Out::Batch`).
    fn send_protos(&mut self, list: &[(i32, i32)]) {
        if list.is_empty() {
            return;
        }
        let conv = self.conv();
        let map = &self.map;
        let encoded: Vec<((i32, i32), Vec<u8>)> = list
            .par_iter()
            .map(|p| {
                let s = &map[p];
                let status = if s.stage == Stage::Decorated { DECORATED } else { TERRAIN };
                (*p, nbtio::encode_root(&conv.proto_nbt(&s.c, status, 0, &s.bes)))
            })
            .collect();
        drop(conv);
        for (p, bytes) in encoded {
            if let Some(s) = self.map.get_mut(&p) {
                s.dirty = false;
                s.sent = self.seq;
            }
            let _ = self.out.send(Out::Proto(p, bytes));
        }
    }

    /// Flush (save, closing, end of a generation without a server): a numbered batch without
    /// generation, which returns the proto-chunks not yet written; its `Out::Batch` makes the
    /// acknowledgement be set after them. No effect on the old path (all returned at every batch).
    pub(crate) fn flush(&mut self) {
        if !self.lazy {
            return;
        }
        let list = self.unwritten();
        if rc_data::env_flag!("RUSTCRAFT_GENLOG") {
            eprintln!("singleplayer: flush of {} proto-chunks", list.len());
        }
        if list.is_empty() {
            return;
        }
        self.seq += 1;
        self.send_protos(&list);
        let _ = self.out.send(Out::Batch(self.seq));
    }

    /// Puts back into the table the evicted chunks of `list` not yet covered by an acknowledgement.
    fn restore(&mut self, list: &[(i32, i32)]) {
        if self.parked.is_empty() {
            return;
        }
        for p in list {
            if self.map.contains_key(p) {
                continue;
            }
            let Some(k) = self.parked.remove(p) else { continue };
            let slot = match k {
                Parked::Proto(s) => s,
                Parked::Full { runs, min_y, height, sent } => {
                    let mut c = DecoChunk::new(p.0, p.1, min_y, height);
                    unpack(&runs, &mut c.blocks);
                    Slot { c: Box::new(c), stage: Stage::Full, disk_full: false, dirty: false, bes: Vec::new(), sent }
                }
            };
            self.map.insert(*p, slot);
        }
    }
}

/// `RUSTCRAFT_OPT_FULLCACHE` (default: on, `0`: scans redone by each lighting):
/// column scans ([`rc_light::scan_column`]) kept for the whole batch.
fn full_cache() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_FULLCACHE", true)
}

/// Scans, in parallel, the columns of `list` that are present and not yet scanned.
fn scan_missing(
    scans: &mut HashMap<(i32, i32), rc_light::ColumnScan>,
    map: &HashMap<(i32, i32), Slot>,
    light: &rc_light::LightTables,
    height: i32,
    list: &[(i32, i32)],
) {
    let todo: Vec<(i32, i32)> = list.iter().copied().filter(|p| !scans.contains_key(p)).collect();
    let made: Vec<((i32, i32), rc_light::ColumnScan)> = todo
        .par_iter()
        .filter_map(|p| {
            let b = &map.get(p)?.c.blocks;
            (b.len() >= 256 * height as usize).then(|| (*p, rc_light::scan_column(light, b, height)))
        })
        .collect();
    scans.extend(made);
}

/// Scans of the 9 columns around `(x, z)` (missing ones: redone by the lighting).
fn scans_of(scans: &HashMap<(i32, i32), rc_light::ColumnScan>, x: i32, z: i32) -> rc_light::Scans<'_> {
    std::array::from_fn(|i| scans.get(&(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1)))
}

/// Lighting of a chunk with the batch's scans. In tests, compared to the lighting redone without
/// scans: a stale scan (blocks changed since) or a misplaced one fails any test that generates.
fn light_scanned(
    light: &rc_light::LightTables,
    cols: rc_light::Columns,
    scans: &HashMap<(i32, i32), rc_light::ColumnScan>,
    (x, z): (i32, i32),
    height: i32,
    has_sky: bool,
) -> (Vec<LightLayer>, Vec<LightLayer>) {
    let out = rc_light::light_chunk_scanned(light, cols, scans_of(scans, x, z), height, has_sky);
    #[cfg(test)]
    if !scans.is_empty() {
        assert!(
            out == rc_light::light_chunk(light, cols, height, has_sky),
            "light of chunk {x} {z}: batch scans differ from the columns"
        );
    }
    out
}

impl Worker {
    /// Rereads from disk the chunks missing from memory (status `terrain` or higher), after those
    /// still waiting for their write.
    fn load(&mut self, list: &[(i32, i32)]) {
        self.restore(list);
        let terrain: Vec<(i32, i32)> = list.iter().copied().filter(|p| !self.map.contains_key(p)).collect();
        {
            let loaded = self.dim.load_raw("region", &terrain);
            let conv = self.conv();
            let read: Vec<ReadOne> = loaded
                .into_par_iter()
                .map(|(p, nbt)| {
                    let saved = nbt.as_ref().and_then(saved_starts_proto);
                    (p, nbt.and_then(|n| conv.from_nbt(&n)).filter(|(c, _, _)| (c.cx, c.cz) == p), saved)
                })
                .collect();
            drop(conv);
            for (p, r, saved) in read {
                let Some((mut c, rank, bes)) = r else {
                    if let Some(s) = saved {
                        self.saved_structures.insert(p, s);
                    }
                    continue;
                };
                // Old converted chunk (read under status `terrain`): band under y = 0
                // regenerated, old blocks kept (rc-worldgen `retro_terrain`).
                let retro = rank < RANK_TERRAIN;
                if retro && let Some(t) = self.dg.retro_terrain(&c) {
                    c = t;
                }
                let stage = if rank >= RANK_FULL {
                    Stage::Full
                } else if rank >= RANK_FEATURES {
                    Stage::Decorated
                } else {
                    Stage::Terrain
                };
                if stage != Stage::Full && crate::mem_stats::enabled() {
                    crate::mem_stats::protos_read(1);
                }
                // Full chunk returned here then saved: always ours (relit with its neighbours).
                let disk_full = stage == Stage::Full && !self.made.contains(&p);
                self.map.insert(p, Slot { c: Box::new(c), stage, disk_full, dirty: retro, bes, sent: 0 });
            }
        }
    }

    /// Decorates the given chunks (at status `terrain`, neighbours ready), class by class.
    fn decorate(&mut self, todo: &[(i32, i32)]) {
        let groups: Vec<Vec<(i32, i32)>> = match self.order {
            DecorOrder::Sequential => todo.iter().rev().map(|&p| vec![p]).collect(),
            o => {
                let mut classes: Vec<i32> = (0..9).collect();
                if o == DecorOrder::ClassesReversed {
                    classes.reverse();
                }
                classes
                    .into_iter()
                    .map(|class| {
                        todo.iter()
                            .copied()
                            .filter(|&(x, z)| (x.rem_euclid(3) + 3 * z.rem_euclid(3)) == class)
                            .collect()
                    })
                    .collect()
            }
        };
        for centers in groups {
            if centers.is_empty() {
                continue;
            }
            let mut jobs: Vec<Job> = centers
                .iter()
                .map(|&(x, z)| {
                    let v = (0..9)
                        .map(|i| {
                            let p = (x + i % 3 - 1, z + i / 3 - 1);
                            std::mem::replace(&mut self.map.get_mut(&p).expect("terrain ready").c, placeholder())
                        })
                        .collect();
                    ((x, z), v)
                })
                .collect();
            let g = &self.dg;
            let stats: Vec<_> = jobs
                .par_iter_mut()
                .map(|(_, v)| {
                    let mut it = v.iter_mut();
                    let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| &mut **it.next().expect("9 chunks"));
                    g.decorate(refs).missing
                })
                .collect();
            for m in stats {
                for (k, v) in m {
                    *self.missing.entry(k).or_insert(0) += v;
                }
            }
            for ((x, z), v) in jobs {
                for (i, c) in v.into_iter().enumerate() {
                    let p = (x + i as i32 % 3 - 1, z + i as i32 / 3 - 1);
                    let s = self.map.get_mut(&p).expect("slot");
                    s.c = c;
                    s.dirty = true;
                }
                if let Some(s) = self.map.get_mut(&(x, z)) {
                    s.stage = s.stage.max(Stage::Decorated);
                }
            }
        }
    }

    /// Eviction: full chunks surrounded by full ones over two rings, then, beyond `KEEP`, the
    /// farthest from the current batch. An evicted one whose latest version is not yet on disk
    /// (server write not acknowledged) is set aside in `parked`, reread from there and not from disk.
    fn evict(&mut self, near: (i32, i32)) {
        // Set aside, now on disk: proto-chunk written, full one saved.
        let written = self.written.load(Ordering::Acquire);
        let saved = self.saved.load(Ordering::Acquire);
        self.parked.retain(|_, k| match k {
            Parked::Proto(s) => s.sent > written,
            Parked::Full { sent, .. } => *sent > saved,
        });
        // Evictable without overlay: full ones within two chunks (a relit neighbour then has no
        // upcoming neighbour). A full chunk returned here counts wherever it is: the result depends on neither the
        // cap nor the saves.
        let (map, parked, made) = (&self.map, &self.parked, &self.made);
        let full = |p: (i32, i32)| map.get(&p).is_some_and(|s| s.stage == Stage::Full) || made.contains(&p);
        let done: Vec<(i32, i32)> = map
            .keys()
            .chain(parked.iter().filter(|(_, k)| matches!(k, Parked::Full { .. })).map(|(p, _)| p))
            .copied()
            .filter(|&(x, z)| (-2..=2).all(|dz| (-2..=2).all(|dx| full((x + dx, z + dz)))))
            .collect();
        for p in done {
            self.map.remove(&p);
            self.parked.remove(&p);
        }
        if self.map.len() > self.keep {
            // Lazy write: a proto-chunk not yet written is evicted too, written at that
            // moment (old path: all written per batch, this filter removed nothing).
            let lazy = self.lazy;
            let mut v: Vec<((i32, i32), i64)> = self
                .map
                .iter()
                .filter(|(_, s)| !s.dirty || (lazy && s.unwritten()))
                .map(|(&(x, z), _)| {
                    let (dx, dz) = (i64::from(x - near.0), i64::from(z - near.1));
                    ((x, z), dx * dx + dz * dz)
                })
                .collect();
            v.sort_by_key(|e| std::cmp::Reverse(e.1));
            let drop_n = self.map.len() - self.keep;
            v.truncate(drop_n);
            let todo: Vec<(i32, i32)> =
                v.iter().map(|e| e.0).filter(|p| self.map.get(p).is_some_and(Slot::unwritten)).collect();
            self.send_protos(&todo);
            for (p, _) in v {
                let Some(s) = self.map.remove(&p) else { continue };
                if s.stage == Stage::Full && !s.disk_full {
                    // Returned to the server: on disk at its next save only.
                    if s.sent > saved {
                        let (min_y, height) = (s.c.min_y, s.c.height);
                        self.parked.insert(p, Parked::Full { runs: pack(&s.c.blocks), min_y, height, sent: s.sent });
                    }
                } else if s.sent > written {
                    self.parked.insert(p, Parked::Proto(s));
                }
            }
        }
    }
}

/// Chunk reread from disk: content, status rank, block entities.
type Read = (DecoChunk, u8, Vec<Nbt>);
/// Region of a decoration: center and its 9 chunks taken out of the table.
type Job = ((i32, i32), Vec<Box<DecoChunk>>);

fn placeholder() -> Box<DecoChunk> {
    Box::new(DecoChunk::new(0, 0, 0, 0))
}

/// Generation without a thread (verification, tools): makes `targets` full in a single batch, in the
/// world `world` (settings read from its `world_gen_settings.dat`); returns the full chunks and
/// the proto-chunks (uncompressed NBT) that would be written.
#[allow(clippy::type_complexity)]
pub fn generate_now(
    data: Arc<VersionData>,
    world: &Path,
    dimension: &str,
    biome_registry: Vec<String>,
    targets: &[(i32, i32)],
    order: DecorOrder,
) -> Result<(Vec<Chunk>, Vec<((i32, i32), Vec<u8>)>), String> {
    let (spec, seed) = read_spec(world, dimension).ok_or("world_gen_settings.dat unreadable")?;
    let (tx, rx) = crossbeam_channel::unbounded();
    let height = crate::dimension_height(&data, dimension);
    let setup =
        Setup { data, world: world.to_owned(), dimension: dimension.to_owned(), height, biome_registry, spec, seed };
    let mut w = Worker::new(setup, tx)?;
    w.order = order;
    w.process(targets);
    w.flush();
    if !w.missing.is_empty() {
        eprintln!("singleplayer: features not reproduced: {:?}", w.missing);
    }
    drop(w);
    let (mut full, mut protos) = (Vec::new(), Vec::new());
    for o in rx.try_iter() {
        match o {
            Out::Full(c, _) => full.push(*c),
            Out::Relight(p, sky, block) => {
                if let Some(c) = full.iter_mut().find(|c: &&mut Chunk| (c.x, c.z) == p) {
                    c.sky = sky;
                    c.block = block;
                }
            }
            Out::Proto(p, n) => protos.push((p, n)),
            Out::Disabled(e) => return Err(e),
            Out::Batch(_) => {}
        }
    }
    Ok((full, protos))
}

/// Generation thread of the open dimension, if the world describes its generator.
pub(crate) fn start_for(
    data: &Arc<VersionData>,
    world: &Path,
    dimension: &str,
    regs: &crate::registries::Registries,
) -> Option<Gen> {
    let (spec, seed) = read_spec(world, dimension)?;
    Gen::start(Setup {
        data: data.clone(),
        world: world.to_owned(),
        dimension: dimension.to_owned(),
        height: crate::dimension_height(data, dimension),
        biome_registry: regs.entries("minecraft:worldgen/biome").to_vec(),
        spec,
        seed,
    })
}

impl crate::server::Server {
    /// Outputs of the generation thread: full chunks put in place (sent through the queue),
    /// proto-chunks written.
    pub(crate) fn poll_gen(&mut self) {
        let Some(g) = &mut self.generator else { return };
        let mut protos = Vec::new();
        let mut disabled = None;
        let mut arrived: Vec<((i32, i32), Vec<Nbt>)> = Vec::new();
        let mut relit: Vec<Relit> = Vec::new();
        while let Ok(o) = g.rx.try_recv() {
            match o {
                Out::Full(c, ents) => {
                    let p = (c.x, c.z);
                    g.pending.remove(&p);
                    self.chunks.insert(p, Some(c));
                    arrived.push((p, ents));
                }
                Out::Relight(p, sky, block) => relit.push((p, sky, block)),
                Out::Proto(p, nbt) => protos.push((p, nbt)),
                Out::Disabled(e) => disabled = Some(e),
                Out::Batch(s) => g.seen = g.seen.max(s),
            }
        }
        // Acknowledgement of the proto-chunks received, set after their writes in the queue.
        let ack = (g.seen > g.acked).then(|| {
            g.acked = g.seen;
            crate::save::Job::Ack(g.written.clone(), g.seen)
        });
        // References of the exploration maps set during the session (cmds_locate).
        for (p, _) in &arrived {
            self.apply_structure_refs(*p);
        }
        if crate::mem_stats::enabled() {
            protos.iter().for_each(|(p, _)| crate::mem_stats::proto_written(*p));
            arrived.iter().for_each(|(p, _)| crate::mem_stats::full_arrived(*p));
        }
        self.gen_arrived(arrived, relit);
        if !protos.is_empty() || ack.is_some() {
            let dir = self.dim.region_dir();
            let jobs = protos
                .into_iter()
                .map(|((cx, cz), nbt)| crate::save::Job::Chunk {
                    path: dir.join(crate::region::region_name(cx, cz)),
                    cx,
                    cz,
                    nbt: self.patch_proto_refs((cx, cz), nbt),
                })
                .chain(ack)
                .collect();
            self.writer.get_or_insert_with(crate::save::Writer::new).send(jobs);
        }
        if let Some(e) = disabled {
            eprintln!("singleplayer: generation impossible: {e}");
            self.generator = None;
            if crate::mem_stats::enabled() {
                crate::mem_stats::forget_gen(&self.dim.name);
            }
        }
    }

    /// Generated chunks arrived: fluid ticks resumed, structure entities adopted;
    /// neighbours already there: new light (returned to the client if it has them).
    pub(crate) fn gen_arrived(&mut self, arrived: Vec<((i32, i32), Vec<Nbt>)>, relit: Vec<Relit>) {
        for (p, ents) in arrived {
            self.import_chunk_ticks(p);
            // "Spawns" step of a new chunk: biome animals.
            self.spawn_original_mobs(p);
            if ents.is_empty() {
                continue;
            }
            let list: Vec<(i32, Nbt)> = ents
                .into_iter()
                .map(|e| {
                    self.next_entity += 1;
                    (self.next_entity, e)
                })
                .collect();
            // Decorations of the new chunk (elytra frame of the End ships...): the chunk has
            // already been adopted empty on request, they are registered here (otherwise the upcoming
            // adoption will take them).
            if self.deco.adopted.contains(&p) {
                for (id, e) in &list {
                    self.deco_register(*id, e);
                }
            }
            self.entities.entry(p).or_default().extend(list);
            self.mobs.adopted.remove(&p);
            self.adopt_mobs(&[p]);
        }
        for (p, sky, block) in relit {
            let Some(Some(c)) = self.chunks.get_mut(&p) else { continue };
            c.sky = sky;
            c.block = block;
            c.dirty = true;
            if self.sent.contains(&p) {
                let light = Box::new(crate::server::light_data(&c.sky, &c.block, self.light_opts.sections));
                let _ = self.send(rc_client::Event::LightUpdate { x: p.0, z: p.1, light });
            }
        }
    }

    /// Save (lazy write): the current batch finishes, then the proto-chunks not yet
    /// written are picked up and put in the write queue, before the rest of the save
    /// (`level.dat`, synchronization). `wait` false (automatic save, pause), by default:
    /// requested only, written at the next pickup (`RUSTCRAFT_OPT_PROTO_FLUSH_ASYNC=0`: awaited).
    pub(crate) fn flush_gen(&mut self, wait: bool) {
        let Some(done) = self.generator.as_ref().and_then(Gen::flush) else { return };
        if !wait && proto_flush_async() {
            return;
        }
        let t = std::time::Instant::now();
        let _ = done.recv();
        self.poll_gen();
        if rc_data::env_flag!("RUSTCRAFT_GENLOG") {
            eprintln!("singleplayer: proto-chunk flush waited {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
        }
    }

    /// Closing: the current batch finishes, its outputs are taken before the final save.
    pub(crate) fn stop_gen(&mut self) {
        if let Some(g) = &mut self.generator {
            g.stop();
        }
        self.poll_gen();
        self.generator = None;
        if crate::mem_stats::enabled() {
            crate::mem_stats::forget_gen(&self.dim.name);
        }
    }
}

/// Chunk reread from disk: position, usable chunk, starts to carry over.
type ReadOne = ((i32, i32), Option<Read>, Option<Nbt>);

/// Structure starts recorded for a "structure_starts" proto-chunk (neither terrain nor converted
/// old chunk): the game rereads it and continues it without recomputing them, references included.
fn saved_starts_proto(root: &Nbt) -> Option<Nbt> {
    let rank = crate::generation_nbt::status_rank(root.get("Status")?.as_str()?)?;
    if !(1..RANK_TERRAIN).contains(&rank) || root.get("below_zero_retrogen").is_some() {
        return None;
    }
    root.get("structures").filter(|s| s.get("starts").is_some()).cloned()
}

#[cfg(test)]
#[path = "generation_tests.rs"]
mod inflight_tests;

#[cfg(test)]
mod saved_starts_tests {
    use super::*;

    fn proto(status: &str, retro: bool) -> Nbt {
        let mut f = vec![
            ("Status".to_owned(), Nbt::String(status.into())),
            (
                "structures".to_owned(),
                Nbt::Compound(vec![("starts".into(), Nbt::Compound(vec![("a".into(), Nbt::Int(1))]))]),
            ),
        ];
        if retro {
            f.push(("below_zero_retrogen".into(), Nbt::Compound(Vec::new())));
        }
        Nbt::Compound(f)
    }

    /// A "structure_starts" proto keeps its starts (map references included);
    /// neither an empty chunk, nor a converted chunk, nor a terrain chunk carries any over.
    #[test]
    fn only_structure_starts_protos_carry_their_starts() {
        assert!(saved_starts_proto(&proto("minecraft:structure_starts", false)).is_some());
        assert!(saved_starts_proto(&proto("minecraft:empty", false)).is_none());
        assert!(saved_starts_proto(&proto("minecraft:structure_starts", true)).is_none());
        assert!(saved_starts_proto(&proto("minecraft:terrain", false)).is_none());
    }
}
