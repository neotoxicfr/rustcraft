//! Saves (`integrated-server.md` § 8, `anvil-world.md` § 7–8): snapshot of the modified data
//! taken on the server thread, compression and writes on a write thread, in the game's
//! order (players → chunks → `level.dat` → `data/`).

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rc_proto::nbt::Nbt;

use crate::chunk::Chunk;
use crate::nbtio::{self, set};
use crate::region::region_name;
use crate::server::Server;

#[path = "save_write_thread.rs"]
mod write_thread;

#[cfg(test)]
pub(crate) use write_thread::{CRASH_BEFORE_RENAME, WriterSabotage, compress_chunks_on};
pub(crate) use write_thread::{WriteThread, WriterPlan, replace, skip_clean_region_sync};

/// Save timings (`RUSTCRAFT_SAVE_TIMINGS`): one line per save for the snapshot's phases on the
/// server thread, one for the write thread's steps. A log, not a switch.
pub(crate) fn timings_enabled() -> bool {
    rc_data::env_flag!("RUSTCRAFT_SAVE_TIMINGS")
}

/// Pool of the save's parallel steps: chunk encoding on the server thread; compression, forced
/// writes and file replacements on the write thread. `RUSTCRAFT_THREADS` threads, otherwise one
/// per logical core. Its own threads rather than rayon's global pool: generation and chunk loading
/// keep that one busy with tasks of several milliseconds, and the server thread, which waits for
/// the encoding, would wait behind them.
pub(crate) fn save_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(crate::storage::worker_threads())
            .thread_name(|i| format!("save-{i}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// Write job.
pub(crate) enum Job {
    /// Chunk (uncompressed NBT) in its region.
    Chunk { path: PathBuf, cx: i32, cz: i32, nbt: Vec<u8> },
    /// `.dat` file (uncompressed NBT, gzip when writing), safe replacement; `backup`:
    /// keep the old one as `<name>_old`.
    Dat { path: PathBuf, nbt: Vec<u8>, backup: bool },
    /// Text file written as is (advancements, statistics: JSON), S9.
    Raw { path: PathBuf, bytes: Vec<u8> },
    /// End of a full save: writes forced to disk.
    Sync,
    /// Save finished: "in progress" flag reset to false.
    Done(std::sync::Arc<std::sync::atomic::AtomicBool>),
    /// Previous writes done: the counter goes to at least this number (acknowledgements returned to
    /// the generation thread, `generation.rs`).
    Ack(std::sync::Arc<std::sync::atomic::AtomicU64>, u64),
}

impl Job {
    /// The file a `.dat` or raw job replaces.
    fn replaced_file(&self) -> Option<&std::path::Path> {
        match self {
            Job::Dat { path, .. } | Job::Raw { path, .. } => Some(path),
            _ => None,
        }
    }
}

pub(crate) struct Writer {
    tx: Option<crossbeam_channel::Sender<Vec<Job>>>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(test)]
thread_local! {
    /// Tests: latency (ms) added before each batch by the writer created from this thread.
    pub(crate) static TEST_WRITER_DELAY_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Latency injected before each write batch (`RUSTCRAFT_TEST_WRITER_DELAY_MS`, read once;
/// robustness trials only, 0 by default).
fn writer_delay() -> std::time::Duration {
    static ENV: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let env = *ENV
        .get_or_init(|| rc_data::env_str!("RUSTCRAFT_TEST_WRITER_DELAY_MS").and_then(|v| v.parse().ok()).unwrap_or(0));
    #[cfg(test)]
    let env = env.max(TEST_WRITER_DELAY_MS.with(std::cell::Cell::get));
    std::time::Duration::from_millis(env)
}

impl Writer {
    pub(crate) fn new() -> Writer {
        Writer::with_plan(WriterPlan::from_switches())
    }

    pub(crate) fn with_plan(plan: WriterPlan) -> Writer {
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<Job>>();
        let delay = writer_delay();
        let thread = std::thread::Builder::new()
            .name("solo-write".into())
            .spawn(move || {
                let mut state = WriteThread::new(plan);
                for jobs in rx {
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    state.run_batch(jobs);
                }
                // Closing: regions padded and forced to disk.
                state.finish();
            })
            .ok();
        Writer { tx: Some(tx), thread }
    }

    pub(crate) fn send(&self, jobs: Vec<Job>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(jobs);
        }
    }

    /// Batches waiting (memory sampling).
    pub(crate) fn queued(&self) -> usize {
        self.tx.as_ref().map_or(0, crossbeam_channel::Sender::len)
    }

    /// Waits for the end of all writes and closes the regions.
    pub(crate) fn finish(self) {
        drop(self);
    }
}

/// Dropped without `finish` (server forgotten without closing): queued writes are awaited too, so that
/// none lands afterwards in an erased or reopened folder (new world at the same path).
impl Drop for Writer {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn doubles(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&d| Nbt::Double(d)).collect())
}

impl Server {
    /// Player file: compound read, managed fields replaced (§ 5).
    pub(crate) fn player_nbt(&self) -> Nbt {
        let mut root = self.pdata.root.clone();
        set(&mut root, "DataVersion", Nbt::Int(crate::chunk::DATA_VERSION));
        set(&mut root, "Pos", doubles(self.pos));
        set(&mut root, "Rotation", Nbt::List(vec![Nbt::Float(self.yaw), Nbt::Float(self.pitch)]));
        set(&mut root, "OnGround", Nbt::Byte(i8::from(self.on_ground)));
        set(&mut root, "Dimension", Nbt::String(self.dim.name.clone()));
        set(&mut root, "playerGameType", Nbt::Int(self.game_mode));
        set(&mut root, "SelectedItemSlot", Nbt::Int(self.held));
        let mut inv = Vec::new();
        for disk in 0..36 {
            if let Some(menu) = crate::player::menu_slot(disk)
                && let Some(s) = &self.slots[menu]
            {
                inv.push(s.to_nbt(Some(disk as i8)));
            }
        }
        set(&mut root, "Inventory", Nbt::List(inv));
        let mut eq = root.get("equipment").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for (k, i) in [("head", 5), ("chest", 6), ("legs", 7), ("feet", 8), ("offhand", 45)] {
            match &self.slots[i] {
                Some(s) => set(&mut eq, k, s.to_nbt(None)),
                None => nbtio::remove(&mut eq, k),
            }
        }
        if matches!(&eq, Nbt::Compound(f) if !f.is_empty()) {
            set(&mut root, "equipment", eq);
        } else {
            nbtio::remove(&mut root, "equipment");
        }
        let mut ab = root.get("abilities").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        let a = self.abilities();
        for (k, v) in [
            ("flying", a.flying),
            ("mayfly", a.may_fly),
            ("instabuild", a.instabuild),
            ("invulnerable", a.invulnerable),
            ("mayBuild", self.game_mode != 2 && self.game_mode != 3),
        ] {
            set(&mut ab, k, Nbt::Byte(i8::from(v)));
        }
        set(&mut ab, "flySpeed", Nbt::Float(a.flying_speed));
        set(&mut ab, "walkSpeed", Nbt::Float(a.walking_speed));
        set(&mut root, "abilities", ab);
        self.life_player_nbt(&mut root); // S2: health, hunger, experience, book, Ender
        self.dims_player_nbt(&mut root); // S6: portal delay, credits seen
        self.proj_player_nbt(&mut root); // S8: ender pearls
        crate::mobs::surface::parrot::save_shoulders(self, &mut root); // F02: parrots
        self.mount_player_nbt(&mut root); // F04: ridden vehicle
        // G3-F08: screen effects, always written (empty list included), like the game.
        if root.get("post_effects").is_none() {
            set(&mut root, "post_effects", Nbt::List(Vec::new()));
        }
        root
    }

    /// `level.dat`: compound read, `Time`, `LastPlayed`, `singleplayer_uuid`, version history
    /// updated (§ 2); nothing else changes (neither `ServerBrands` nor `WasModded`).
    fn level_nbt(&self) -> Nbt {
        let mut root = self.level.root.clone();
        let Some(d) = nbtio::get_mut(&mut root, "Data") else { return root };
        set(d, "Time", Nbt::Long(self.time));
        let now = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64);
        set(d, "LastPlayed", Nbt::Long(now));
        set(d, "singleplayer_uuid", nbtio::uuid_nbt(self.uuid));
        let v = crate::chunk::DATA_VERSION;
        match nbtio::get_mut(d, "version_history") {
            Some(Nbt::List(l)) => {
                if !l.iter().any(|x| x.as_i64() == Some(i64::from(v))) {
                    l.push(Nbt::Int(v));
                }
            }
            _ => set(d, "version_history", Nbt::List(vec![Nbt::Int(v)])),
        }
        root
    }

    /// `world_clocks.dat` with the current clocks (only if they advanced).
    fn clocks_nbt(&self) -> Option<Nbt> {
        if self.clocks == self.clocks_loaded {
            return None;
        }
        let path = self.world.join("data").join("minecraft").join("world_clocks.dat");
        let mut root = nbtio::read_dat(&path).unwrap_or_else(|_| Nbt::Compound(Vec::new()));
        let mut d = root.get("data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for (name, total) in &self.clocks {
            let mut c = d.get(name).cloned().unwrap_or(Nbt::Compound(Vec::new()));
            set(&mut c, "total_ticks", Nbt::Long(*total));
            set(&mut d, name, c);
        }
        set(&mut root, "data", d);
        set(&mut root, "DataVersion", Nbt::Int(crate::chunk::DATA_VERSION));
        Some(root)
    }

    /// Modified chunks of the active dimension: write jobs; returns their count.
    pub(crate) fn chunk_jobs(&mut self, jobs: &mut Vec<Job>) -> usize {
        self.chunk_jobs_with(jobs, EncodePlan::from_switches(), save_pool())
    }

    /// [`Server::chunk_jobs`] with the plan and pool given (proofs and benches).
    pub(crate) fn chunk_jobs_with(
        &mut self,
        jobs: &mut Vec<Job>,
        plan: EncodePlan,
        pool: Option<&rayon::ThreadPool>,
    ) -> usize {
        let region_dir = self.dim.region_dir();
        let mut dirty: Vec<(i32, i32)> =
            self.chunks.iter().filter(|(_, c)| c.as_ref().is_some_and(|c| c.dirty)).map(|(p, _)| *p).collect();
        dirty.sort_unstable();
        let encoded = match pool.filter(|_| plan.parallel && dirty.len() > 1) {
            Some(pool) => self.encode_in_parallel(&dirty, pool, plan),
            None => self.encode_in_series(&dirty),
        };
        let mut written = 0;
        for (p, nbt) in encoded {
            let Some(nbt) = nbt else {
                eprintln!("singleplayer: chunk {p:?} not rewritten (unknown identifier)");
                continue;
            };
            jobs.push(Job::Chunk { path: region_dir.join(region_name(p.0, p.1)), cx: p.0, cz: p.1, nbt });
            written += 1;
        }
        // Full chunks received from the generator up to batch `seen`: written above or before.
        if let Some(g) = &self.generator {
            jobs.push(Job::Ack(g.saved.clone(), g.seen));
            // Block changes sent to the generation thread so far: written above or before (a raw
            // section write is not marked for saving, so it may never be, as in the game).
            jobs.push(Job::Ack(g.block_changes.saved.clone(), g.block_changes.last_sent()));
        }
        written
    }

    /// The block entities kept packed promoted (L9-TICK) and the scheduled ticks exported into
    /// the chunk's compound: both read and change server state beyond the chunk.
    fn prepare_for_save(&mut self, p: (i32, i32)) {
        self.promote_packed_for_save(p);
        self.export_chunk_ticks(p);
    }

    /// The old path: each chunk prepared then encoded, one after the other.
    fn encode_in_series(&mut self, dirty: &[(i32, i32)]) -> Vec<EncodedChunk> {
        let (h, time) = (self.dim.height, self.time);
        let mut encoder = nbtio::RootEncoder::default();
        let mut out = Vec::with_capacity(dirty.len());
        for &p in dirty {
            let t = Instant::now();
            self.prepare_for_save(p);
            let prepared = Instant::now();
            add_chunk_time(|c| &mut c.prepare, prepared - t);
            let Some(Some(c)) = self.chunks.get_mut(&p) else { continue };
            out.push((p, encode_one(c, (&*self.data, &*self.states, h, time), &mut encoder)));
            add_chunk_time(|c| &mut c.encode, prepared.elapsed());
        }
        out
    }

    /// Every chunk prepared in series, then encoded side by side on `pool`; the results keep the
    /// sorted order, so the jobs and the messages come out as on the old path.
    fn encode_in_parallel(
        &mut self,
        dirty: &[(i32, i32)],
        pool: &rayon::ThreadPool,
        plan: EncodePlan,
    ) -> Vec<EncodedChunk> {
        let t = Instant::now();
        for &p in dirty {
            self.prepare_for_save(p);
        }
        let prepared = Instant::now();
        add_chunk_time(|c| &mut c.prepare, prepared - t);
        // Only the chunks listed before the preparation (it may mark others).
        let mut chunks: Vec<((i32, i32), &mut Chunk)> = self
            .chunks
            .iter_mut()
            .filter(|(p, _)| dirty.binary_search(p).is_ok())
            .filter_map(|(p, c)| Some((*p, c.as_deref_mut()?)))
            .collect();
        chunks.sort_unstable_by_key(|(p, _)| *p);
        let context = (&*self.data, &*self.states, self.dim.height, self.time);
        let encoded: Vec<EncodedChunk> = pool.install(|| {
            chunks
                .into_par_iter()
                .map_init(nbtio::RootEncoder::default, |encoder, (p, c)| {
                    let nbt = encode_one(c, context, encoder);
                    #[cfg(test)]
                    if plan.sabotage == EncodeSabotage::RootNotGivenBack {
                        c.root = Nbt::Compound(Vec::new());
                    }
                    (p, nbt)
                })
                .collect()
        });
        add_chunk_time(|c| &mut c.encode, prepared.elapsed());
        #[cfg(test)]
        if plan.sabotage == EncodeSabotage::ReversedOrder {
            return encoded.into_iter().rev().collect();
        }
        let _ = plan; // read by the test sabotages only
        encoded
    }

    /// Full save (§ 8). `wait`: closing, writes awaited and regions closed.
    pub(crate) fn save(&mut self, wait: bool) {
        let t = Instant::now();
        let mut phases = SnapshotPhases::starting_at(t);
        let (jobs, written) = self.save_jobs(wait, region_sync_only_at_closing(), &mut phases);
        let snapshot = t.elapsed();
        let writer = self.writer.get_or_insert_with(Writer::new);
        writer.send(jobs);
        if wait && let Some(w) = self.writer.take() {
            w.finish();
        }
        eprintln!(
            "singleplayer: {}save: {written} chunks, snapshot {:.2} ms{}",
            if wait { "closing " } else { "" },
            snapshot.as_secs_f64() * 1e3,
            if wait { format!(", total {:.1} ms", t.elapsed().as_secs_f64() * 1e3) } else { String::new() }
        );
        phases.report();
    }

    /// The snapshot: every job of a full save in the game's order, and the number of chunks.
    /// `sync_only_at_closing`: see [`region_sync_only_at_closing`].
    pub(crate) fn save_jobs(
        &mut self,
        wait: bool,
        sync_only_at_closing: bool,
        phases: &mut SnapshotPhases,
    ) -> (Vec<Job>, usize) {
        self.saving.store(true, std::sync::atomic::Ordering::Release);
        // Proto-chunks of the generator not yet written: in the queue before everything else.
        self.flush_gen(wait);
        phases.lap("generator");
        let mut jobs = Vec::new();
        self.life_save_jobs(&mut jobs); // S2: furnaces, random sequences, weather
        self.commands_save_jobs(&mut jobs); // S5: rules, scores, storage, forced chunks
        self.maps_save_jobs(&mut jobs); // S8: maps
        self.progress_save_jobs(&mut jobs); // S9: advancements, statistics (including "time since rest")
        self.lan_save_jobs(&mut jobs); // S9: LAN players
        self.raids_save_jobs(&mut jobs); // F07: raids.dat
        self.end_save_jobs(&mut jobs); // F15: dragon fight
        phases.lap("data files");
        let player_path = self.pdata.path.clone();
        jobs.push(Job::Dat { path: player_path, nbt: nbtio::encode_root(&self.player_nbt()), backup: true });
        phases.lap("player");
        let mut written = self.chunk_jobs(&mut jobs);
        phases.lap("chunks");
        written += self.save_parked(&mut jobs, wait); // S6: other dimensions
        phases.lap("other dimensions");
        if wait && sync_only_at_closing {
            // Like the game at closing: the regions forced to disk before `level.dat` is replaced.
            jobs.push(Job::Sync);
        }
        jobs.push(Job::Dat {
            path: self.world.join("level.dat"),
            nbt: nbtio::encode_root(&self.level_nbt()),
            backup: true,
        });
        if let Some(c) = self.clocks_nbt() {
            let path = self.world.join("data").join("minecraft").join("world_clocks.dat");
            jobs.push(Job::Dat { path, nbt: nbtio::encode_root(&c), backup: false });
            self.clocks_loaded = self.clocks.clone();
        }
        if !sync_only_at_closing {
            jobs.push(Job::Sync);
        }
        jobs.push(Job::Done(self.saving.clone()));
        phases.lap("level");
        (jobs, written)
    }
}

/// Autosave and pause leave the region files to the system, like the game, which forces them only
/// at closing, before `level.dat` (`.dat` files stay forced at every save, like the game's).
/// `RUSTCRAFT_OPT_REGION_SYNC_ONLY_AT_CLOSING=0`: every save forces them, after `level.dat` (old path).
fn region_sync_only_at_closing() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_SYNC_ONLY_AT_CLOSING")
}

/// How the save encodes the modified chunks (`RUSTCRAFT_OPT_PARALLEL_SAVE_ENCODE`, read once;
/// proofs and benches build their own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EncodePlan {
    /// Chunks encoded side by side on the save pool; what changes server state beyond the chunk
    /// runs in series first.
    pub parallel: bool,
    /// Tests: what the proofs break on purpose.
    #[cfg(test)]
    pub sabotage: EncodeSabotage,
}

/// Deliberate faults of the parallel encoding, to check that the proofs see them.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EncodeSabotage {
    None,
    /// The encoded chunks handed over in reverse order.
    ReversedOrder,
    /// The compound a chunk keeps after its save left empty instead of given back.
    RootNotGivenBack,
}

impl EncodePlan {
    pub(crate) fn from_switches() -> EncodePlan {
        EncodePlan {
            parallel: rc_data::opt_switch!("RUSTCRAFT_OPT_PARALLEL_SAVE_ENCODE"),
            #[cfg(test)]
            sabotage: EncodeSabotage::None,
        }
    }
}

/// A chunk's position and its encoded compound (`None`: never rewritten, unknown identifier).
type EncodedChunk = ((i32, i32), Option<Vec<u8>>);

/// What encoding a chunk reads besides the chunk: data, states, height, game time.
type EncodeContext<'a> = (&'a rc_data::VersionData, &'a crate::states::States, crate::chunk::Height, i64);

/// Encodes one chunk for the save; it no longer counts as modified.
fn encode_one(
    c: &mut Chunk,
    (data, states, h, time): EncodeContext<'_>,
    encoder: &mut nbtio::RootEncoder,
) -> Option<Vec<u8>> {
    c.dirty = false;
    let nbt = c.encode_for_save(data, states, h, time, |root| encoder.encode(root))?;
    c.changed.iter_mut().chain(c.biomes_changed.iter_mut()).for_each(|x| *x = false);
    Some(nbt)
}

/// Time the snapshot spent on chunks since the start of the save (save timings).
#[derive(Debug, Default, Clone, Copy)]
struct ChunkTimes {
    /// Block entity promotion and tick export.
    prepare: Duration,
    encode: Duration,
}

thread_local! {
    static CHUNK_TIMES: std::cell::Cell<ChunkTimes> = std::cell::Cell::new(ChunkTimes::default());
}

fn add_chunk_time(part: impl FnOnce(&mut ChunkTimes) -> &mut Duration, d: Duration) {
    let mut times = CHUNK_TIMES.get();
    *part(&mut times) += d;
    CHUNK_TIMES.set(times);
}

/// Durations of the snapshot's phases on the server thread (save timings).
pub(crate) struct SnapshotPhases {
    last: Instant,
    phases: Vec<(&'static str, Duration)>,
}

impl SnapshotPhases {
    pub(crate) fn starting_at(t: Instant) -> SnapshotPhases {
        CHUNK_TIMES.take();
        SnapshotPhases { last: t, phases: Vec::new() }
    }

    fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        self.phases.push((name, now - self.last));
        self.last = now;
    }

    fn report(&self) {
        if !timings_enabled() {
            return;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let chunks = CHUNK_TIMES.get();
        let phases: Vec<String> = self.phases.iter().map(|(name, d)| format!("{name} {:.2}", ms(*d))).collect();
        eprintln!(
            "singleplayer: save snapshot ms: {} (chunks of every dimension: prepare {:.2}, encode {:.2})",
            phases.join(", "),
            ms(chunks.prepare),
            ms(chunks.encode)
        );
    }
}

#[cfg(test)]
#[path = "save_tests.rs"]
mod tests;

#[cfg(test)]
mod writer_tests {
    use super::*;

    /// Writer dropped without `finish` (server forgotten without closing, like the simulations of the
    /// tests): its queued writes are done before the drop returns control. Otherwise they fell later
    /// into the folder, possibly into the next new world at the same path.
    #[test]
    fn drop_waits_for_queued_writes() {
        TEST_WRITER_DELAY_MS.with(|d| d.set(200));
        let w = Writer::new();
        TEST_WRITER_DELAY_MS.with(|d| d.set(0));
        let n = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        w.send(vec![Job::Ack(n.clone(), 7)]);
        drop(w);
        assert_eq!(n.load(std::sync::atomic::Ordering::Acquire), 7, "writes still in progress after the drop");
    }
}
