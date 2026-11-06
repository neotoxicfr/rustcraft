//! Bases of the partial remeshes: the last mesh received for each column that changes, kept flat
//! (`rc_mesh::blocks::pack`) so that a change remeshes only the sections that read it.
//!
//! The hot stage holds the flat words as they are, within a byte budget, oldest use evicted first.
//! Switches, read once at startup (`docs/perf.md`, C2-M03), `=0` restores the old path:
//! - `RUSTCRAFT_OPT_MESHCACHEADAPTIVE`: hot budget raised while evictions cause misses, lowered
//!   after a long rest ([`BudgetRegulator`]); `RUSTCRAFT_MESHCACHE_MB` fixes the budget and wins.
//! - `RUSTCRAFT_OPT_MESHCACHEEVICT`: segmented eviction, the bases of columns remeshed twice in a
//!   row with a base are protected from a burst of one-off remeshes.
//! - `RUSTCRAFT_OPT_MESHCACHECOLD`: an evicted base is compressed on the pool into a cold stage
//!   (about 4.6 times smaller, `RUSTCRAFT_MESHCACHE_COLD_MB` of compressed bytes), decoded by the
//!   meshing task that needs it again (on the pool, not the main thread).
//! - `RUSTCRAFT_OPT_MESH_BASE_EXPIRY`: bases unused for `RUSTCRAFT_MESH_BASE_EXPIRY_S` seconds
//!   dropped from both stages.
//! - `RUSTCRAFT_OPT_MESH_BASE_SECOND_ADMISSION` (off by default, `=1`): the first live mesh of a
//!   column is not kept, only the next ones (fewer bases for columns changed once).
//!
//! None of them changes a mesh: a base served is always the last mesh received for its column,
//! word for word; only the number of partial remeshes that find no base (whole column) moves.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hashbrown::{HashMap, HashSet};
use rc_mesh::blocks::{BlockMesh, Packed};
use rc_mesh::packed_codec::{self, CompressedMesh};

use super::{PackAt, opts};

type Key = (i32, i32);

/// Hot budget when nothing else sets it, and floor of the regulator (MB). Bench of 19 October
/// (`docs/perf.md`): plains32 private memory 2,360 / 2,499 / 2,588 MB at 32 / 64 / 128 MB, caves
/// pool 497 / 370 / 278 ms/s.
const DEFAULT_BUDGET_MB: usize = 64;
/// Compressed bytes of the cold stage (MB): about 150 MB of flat bases at the measured ratio.
const DEFAULT_COLD_MB: usize = 32;
/// Idle time after which a base expires (s).
const DEFAULT_EXPIRY_S: u64 = 120;
/// Length of a counting window of the regulator and of the expiry.
const WINDOW: Duration = Duration::from_secs(2);

/// Switches and sizes of the cache, read once at startup (set field by field by the tests).
#[derive(Debug, Clone)]
pub(super) struct CachePolicy {
    /// Hot budget in bytes (the floor of the regulator when it is on); 0: no cache.
    pub budget: usize,
    pub adaptive: bool,
    pub frequency_eviction: bool,
    /// Compressed bytes of the cold stage; `None`: no cold stage.
    pub cold_budget: Option<usize>,
    pub expiry: Option<Duration>,
    pub second_admission: bool,
}

impl CachePolicy {
    fn from_env() -> &'static CachePolicy {
        static POLICY: std::sync::OnceLock<CachePolicy> = std::sync::OnceLock::new();
        POLICY.get_or_init(|| {
            let number = |v: Option<&str>| v.and_then(|v| v.parse::<usize>().ok());
            let fixed_mb = number(rc_data::env_str!("RUSTCRAFT_MESHCACHE_MB"));
            let cold_mb = number(rc_data::env_str!("RUSTCRAFT_MESHCACHE_COLD_MB")).unwrap_or(DEFAULT_COLD_MB);
            let expiry_s =
                number(rc_data::env_str!("RUSTCRAFT_MESH_BASE_EXPIRY_S")).map_or(DEFAULT_EXPIRY_S, |s| s as u64);
            let policy = CachePolicy {
                budget: fixed_mb.unwrap_or(DEFAULT_BUDGET_MB) << 20,
                // A budget fixed by hand wins over the regulator.
                adaptive: fixed_mb.is_none() && rc_data::opt_switch!("RUSTCRAFT_OPT_MESHCACHEADAPTIVE"),
                frequency_eviction: rc_data::opt_switch!("RUSTCRAFT_OPT_MESHCACHEEVICT"),
                cold_budget: rc_data::opt_switch!("RUSTCRAFT_OPT_MESHCACHECOLD").then_some(cold_mb << 20),
                expiry: rc_data::opt_switch!("RUSTCRAFT_OPT_MESH_BASE_EXPIRY").then(|| Duration::from_secs(expiry_s)),
                second_admission: rc_data::opt_switch!("RUSTCRAFT_OPT_MESH_BASE_SECOND_ADMISSION", default_off),
            };
            eprintln!("[opt] mesh base cache: {policy:?}");
            policy
        })
    }
}

/// Hot budget that follows the misses caused by evictions: one step (half the floor) up per
/// window that sees enough of them, up to twice the floor, three times while they stay twice as
/// many; one step down after a long rest, never below the floor.
#[derive(Debug, Clone)]
pub(super) struct BudgetRegulator {
    floor: usize,
    budget: usize,
    rest_windows: u32,
}

impl BudgetRegulator {
    /// Misses after an eviction in one window that raise the budget: 3 per second (plains32 sees
    /// 0.6 per second, where a larger cache costs memory for nothing).
    const RAISE_AT: u32 = 6;
    /// Base requests in a window under which it counts as rest.
    const REST_BELOW: u32 = 2;
    /// Rest windows in a row before a step down (30 s).
    const REST_WINDOWS: u32 = 15;

    pub fn new(floor: usize) -> Self {
        BudgetRegulator { floor, budget: floor, rest_windows: 0 }
    }

    /// End of a window: its misses after an eviction and its base requests; new budget.
    pub fn end_window(&mut self, evicted_misses: u32, requests: u32) -> usize {
        let step = self.floor / 2;
        if evicted_misses >= Self::RAISE_AT {
            self.rest_windows = 0;
            let cap = if evicted_misses >= 2 * Self::RAISE_AT { 3 * self.floor } else { 2 * self.floor };
            if self.budget < cap {
                self.budget = (self.budget + step).min(cap);
            }
        } else if requests < Self::REST_BELOW {
            self.rest_windows += 1;
            if self.rest_windows >= Self::REST_WINDOWS {
                self.rest_windows = 0;
                self.budget = self.budget.saturating_sub(step).max(self.floor);
            }
        } else {
            self.rest_windows = 0;
        }
        self.budget
    }
}

/// Base kept flat, with its usage rank.
struct HotEntry {
    packed: Packed,
    version: u64,
    stamp: u64,
    /// Bytes of fluid faces in it (27 words each, against 6 for a face).
    fluid_bytes: usize,
    protected: bool,
    received: Instant,
    used: Instant,
}

/// Base compressed by the cold stage.
struct ColdEntry {
    compressed: Arc<CompressedMesh>,
    version: u64,
    stamp: u64,
    received: Instant,
    used: Instant,
}

/// Evicted base handed to the pool for compression, served as it is until its result comes back.
struct Compressing {
    packed: Packed,
    version: u64,
    /// Number of this compression (never reused, even across worlds where versions start again).
    job: u64,
    received: Instant,
    used: Instant,
}

/// Cold stage: bases evicted from the hot one, compressed (`RUSTCRAFT_OPT_MESHCACHECOLD`).
struct ColdStage {
    budget: usize,
    entries: HashMap<Key, ColdEntry>,
    order: BTreeMap<u64, Key>,
    bytes: usize,
    compressing: HashMap<Key, Compressing>,
    next_job: u64,
    finished_tx: crossbeam_channel::Sender<(Key, u64, CompressedMesh)>,
    finished_rx: crossbeam_channel::Receiver<(Key, u64, CompressedMesh)>,
}

impl ColdStage {
    fn new(budget: usize) -> Self {
        let (finished_tx, finished_rx) = crossbeam_channel::unbounded();
        ColdStage {
            budget,
            entries: HashMap::new(),
            order: BTreeMap::new(),
            bytes: 0,
            compressing: HashMap::new(),
            next_job: 0,
            finished_tx,
            finished_rx,
        }
    }

    /// Columns held, compressed or being compressed.
    fn len(&self) -> usize {
        self.entries.len() + self.compressing.len()
    }

    /// Base evicted from the hot stage: compressed on the pool (the main thread only hands over
    /// the shared words).
    fn start(&mut self, key: Key, e: HotEntry) {
        self.next_job += 1;
        let job = self.next_job;
        let (words, tx) = (e.packed.clone(), self.finished_tx.clone());
        rayon::spawn(move || {
            let t = Instant::now();
            let compressed = packed_codec::compress(&words);
            crate::perf_log::cold_compress_work(t.elapsed());
            let _ = tx.send((key, job, compressed));
        });
        let c = Compressing { packed: e.packed, version: e.version, job, received: e.received, used: e.used };
        self.compressing.insert(key, c);
    }

    /// Compressions finished: kept if their column still waits for them, then the oldest dropped
    /// past the budget (`evicted`).
    fn collect_finished(&mut self, clock: &mut u64, evicted: &mut HashSet<Key>) {
        while let Ok((key, job, compressed)) = self.finished_rx.try_recv() {
            self.accept(key, job, compressed, clock);
        }
        self.drop_over_budget(evicted);
    }

    /// Compression `job` of column `key` back: kept if the column still waits for this one.
    fn accept(&mut self, key: Key, job: u64, compressed: CompressedMesh, clock: &mut u64) {
        if self.compressing.get(&key).is_none_or(|c| c.job != job) {
            return;
        }
        let Some(c) = self.compressing.remove(&key) else { return };
        *clock += 1;
        self.bytes += compressed.bytes_held();
        self.order.insert(*clock, key);
        let (version, received, used) = (c.version, c.received, c.used);
        self.entries
            .insert(key, ColdEntry { compressed: Arc::new(compressed), version, stamp: *clock, received, used });
    }

    fn drop_over_budget(&mut self, evicted: &mut HashSet<Key>) {
        while self.bytes > self.budget {
            let Some((_, key)) = self.order.pop_first() else { break };
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.compressed.bytes_held();
                evicted.insert(key);
            }
        }
    }

    /// Base of the column if held: (words, version, instant received).
    fn serve(&mut self, key: Key, clock: &mut u64, now: Instant) -> Option<(Packed, u64, Instant)> {
        if let Some(c) = self.compressing.get_mut(&key) {
            c.used = now;
            return Some((c.packed.clone(), c.version, c.received));
        }
        let e = self.entries.get_mut(&key)?;
        self.order.remove(&e.stamp);
        *clock += 1;
        e.stamp = *clock;
        e.used = now;
        self.order.insert(e.stamp, key);
        Some((packed_codec::decoded_on_first_read(e.compressed.clone()), e.version, e.received))
    }

    /// Column removed; true if it was held.
    fn take(&mut self, key: Key) -> bool {
        if self.compressing.remove(&key).is_some() {
            return true;
        }
        let Some(e) = self.entries.remove(&key) else { return false };
        self.order.remove(&e.stamp);
        self.bytes -= e.compressed.bytes_held();
        true
    }

    /// Bases unused since `max_idle` dropped (`evicted`).
    fn expire(&mut self, now: Instant, max_idle: Duration, evicted: &mut HashSet<Key>) {
        let idle = |used: Instant| now.saturating_duration_since(used) >= max_idle;
        let mut old: Vec<Key> = self.entries.iter().filter(|(_, e)| idle(e.used)).map(|(k, _)| *k).collect();
        old.extend(self.compressing.iter().filter(|(_, c)| idle(c.used)).map(|(k, _)| *k));
        for key in old {
            self.take(key);
            evicted.insert(key);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
        // Compressions in flight come back with a job number nobody waits for any more.
        self.compressing.clear();
    }

    /// Test only: waits for the compressions in flight.
    #[cfg(test)]
    fn wait_compressions(&mut self, clock: &mut u64, evicted: &mut HashSet<Key>) {
        while !self.compressing.is_empty() {
            let (key, job, compressed) = self.finished_rx.recv().expect("sender held by the stage");
            self.accept(key, job, compressed, clock);
        }
        self.drop_over_budget(evicted);
    }
}

/// Requests of the current window (regulator).
#[derive(Default)]
struct MissWindow {
    started: Option<Instant>,
    requests: u32,
    evicted_misses: u32,
}

/// Bases of the partial remeshes (see the module).
pub(super) struct BaseCache {
    policy: CachePolicy,
    hot: HashMap<Key, HotEntry>,
    /// Usage order of the hot entries (stamp → column), oldest first. With the frequency-aware
    /// eviction, `protected` holds the bases of columns remeshed twice in a row with a base and
    /// `probation` the others (evicted first); otherwise everything is in `probation`.
    probation: BTreeMap<u64, Key>,
    protected: BTreeMap<u64, Key>,
    hot_bytes: usize,
    protected_bytes: usize,
    fluid_bytes: usize,
    /// Hot budget in force (moved by the regulator).
    budget: usize,
    regulator: Option<BudgetRegulator>,
    clock: u64,
    cold: Option<ColdStage>,
    /// Columns whose last mesh is a base the cache dropped (budget or expiry): tells an evicted
    /// base from one never kept (perf log, regulator).
    evicted: HashSet<Key>,
    /// Second admission: columns whose last mesh is their first live one, not kept.
    first_live: HashSet<Key>,
    /// Flattening buffer reused (`PackAt::Copy`).
    scratch: Vec<u32>,
    window: MissWindow,
}

impl Default for BaseCache {
    fn default() -> Self {
        BaseCache::new(CachePolicy::from_env().clone())
    }
}

impl BaseCache {
    pub fn new(policy: CachePolicy) -> Self {
        BaseCache {
            budget: policy.budget,
            regulator: policy.adaptive.then(|| BudgetRegulator::new(policy.budget)),
            cold: policy.cold_budget.map(ColdStage::new),
            policy,
            hot: HashMap::new(),
            probation: BTreeMap::new(),
            protected: BTreeMap::new(),
            hot_bytes: 0,
            protected_bytes: 0,
            fluid_bytes: 0,
            clock: 0,
            evicted: HashSet::new(),
            first_live: HashSet::new(),
            scratch: Vec::new(),
            window: MissWindow::default(),
        }
    }

    /// Mesh received. `live`: redone for changed blocks or light; only such a mesh is kept (the bulk
    /// of the columns, meshed whole at loading, never changes). Whatever it is, it replaces the
    /// column's base. `packed`: already flattened by the pool.
    pub fn put(&mut self, key: Key, m: &BlockMesh, live: bool, packed: Option<Packed>, now: Instant) {
        let had_base = self.take(key);
        let first_live_before = self.first_live.remove(&key);
        if live {
            if self.policy.second_admission && !had_base && !first_live_before {
                self.first_live.insert(key);
            } else {
                let packed = packed.unwrap_or_else(|| self.flatten(m));
                self.keep(key, packed, m, had_base, now);
            }
        }
        self.report();
    }

    fn flatten(&mut self, m: &BlockMesh) -> Packed {
        if opts().pack == PackAt::Copy {
            rc_mesh::blocks::pack(m, &mut self.scratch);
            Arc::new(self.scratch.clone())
        } else {
            Arc::new(rc_mesh::blocks::pack_exact(m))
        }
    }

    /// New hot entry (unless larger than the whole budget), then evictions down to the budget.
    /// `remeshed_with_base`: the column's previous mesh was a base too (protected segment).
    fn keep(&mut self, key: Key, packed: Packed, m: &BlockMesh, remeshed_with_base: bool, now: Instant) {
        let size = packed.len() * 4;
        if size > self.budget {
            return;
        }
        let fluid_bytes = packed_fluid_bytes(m, packed.len());
        let protected = self.policy.frequency_eviction && remeshed_with_base;
        self.clock += 1;
        let stamp = self.clock;
        self.segment(protected).insert(stamp, key);
        let entry = HotEntry {
            packed,
            version: m.version,
            stamp: self.clock,
            fluid_bytes,
            protected,
            received: now,
            used: now,
        };
        self.hot.insert(key, entry);
        self.hot_bytes += size;
        self.fluid_bytes += fluid_bytes;
        if protected {
            self.protected_bytes += size;
            self.demote_past_protected_share();
        }
        self.evict_over_budget();
    }

    fn segment(&mut self, protected: bool) -> &mut BTreeMap<u64, Key> {
        if protected { &mut self.protected } else { &mut self.probation }
    }

    /// The protected segment holds at most three quarters of the budget, so that new columns
    /// still find room: its oldest entries go back to probation (as the most recent there).
    fn demote_past_protected_share(&mut self) {
        while self.protected_bytes > self.budget / 4 * 3 {
            let Some((_, key)) = self.protected.pop_first() else { break };
            let Some(e) = self.hot.get_mut(&key) else { continue };
            self.clock += 1;
            e.stamp = self.clock;
            e.protected = false;
            self.protected_bytes -= e.packed.len() * 4;
            self.probation.insert(self.clock, key);
        }
    }

    /// Oldest bases dropped from the hot stage (into the cold one if it is on) until it fits its
    /// budget; probation before protected.
    fn evict_over_budget(&mut self) {
        while self.hot_bytes > self.budget {
            let Some((_, key)) = self.probation.pop_first().or_else(|| self.protected.pop_first()) else { break };
            let Some(e) = self.remove_hot_entry(key) else { continue };
            match &mut self.cold {
                Some(cold) => cold.start(key, e),
                None => {
                    self.evicted.insert(key);
                }
            }
        }
    }

    /// Hot entry removed (bytes given back); its stamp is already out of the usage order or is
    /// removed by the caller.
    fn remove_hot_entry(&mut self, key: Key) -> Option<HotEntry> {
        let e = self.hot.remove(&key)?;
        let size = e.packed.len() * 4;
        self.hot_bytes -= size;
        self.fluid_bytes -= e.fluid_bytes;
        if e.protected {
            self.protected_bytes -= size;
        }
        Some(e)
    }

    /// Column removed from every stage; true if its last mesh was a base (kept or evicted).
    fn take(&mut self, key: Key) -> bool {
        let hot = self.hot.get(&key).map(|e| (e.stamp, e.protected));
        if let Some((stamp, protected)) = hot {
            self.segment(protected).remove(&stamp);
            self.remove_hot_entry(key);
        }
        let cold = self.cold.as_mut().is_some_and(|c| c.take(key));
        let evicted = self.evicted.remove(&key);
        hot.is_some() || cold || evicted
    }

    /// Base of the column, flat (usage refreshed), and its version.
    pub fn base(&mut self, key: Key, now: Instant) -> Option<(Packed, u64)> {
        self.collect_compressions();
        let served = match self.serve_hot(key, now) {
            Some(s) => Some((s, false)),
            None => self.cold.as_mut().and_then(|c| c.serve(key, &mut self.clock, now)).map(|s| (s, true)),
        };
        self.window.requests += 1;
        match &served {
            Some(((_, _, received), cold)) => {
                crate::perf_log::mesh_base_served(now.saturating_duration_since(*received), *cold)
            }
            None if self.evicted.contains(&key) => self.window.evicted_misses += 1,
            None => {}
        }
        served.map(|((packed, version, _), _)| (packed, version))
    }

    fn serve_hot(&mut self, key: Key, now: Instant) -> Option<(Packed, u64, Instant)> {
        let e = self.hot.get_mut(&key)?;
        let order = if e.protected { &mut self.protected } else { &mut self.probation };
        order.remove(&e.stamp);
        self.clock += 1;
        e.stamp = self.clock;
        e.used = now;
        order.insert(self.clock, key);
        Some((e.packed.clone(), e.version, e.received))
    }

    /// The column's last mesh was a base that the cache dropped.
    pub fn was_evicted(&self, key: Key) -> bool {
        self.evicted.contains(&key)
    }

    fn collect_compressions(&mut self) {
        if let Some(cold) = &mut self.cold {
            cold.collect_finished(&mut self.clock, &mut self.evicted);
        }
    }

    /// Once per frame: finished compressions collected; at the end of each window, budget of the
    /// regulator and expiry.
    pub fn tick(&mut self, now: Instant) {
        self.collect_compressions();
        let started = *self.window.started.get_or_insert(now);
        if now.saturating_duration_since(started) < WINDOW {
            return;
        }
        let window = std::mem::replace(&mut self.window, MissWindow { started: Some(now), ..MissWindow::default() });
        if let Some(r) = &mut self.regulator {
            self.budget = r.end_window(window.evicted_misses, window.requests);
        }
        if let Some(max_idle) = self.policy.expiry {
            self.expire(now, max_idle);
        }
        self.evict_over_budget();
        self.report();
    }

    /// Bases unused since `max_idle` dropped from both stages (not compressed: they would expire
    /// there right away).
    fn expire(&mut self, now: Instant, max_idle: Duration) {
        let old: Vec<Key> = self
            .hot
            .iter()
            .filter(|(_, e)| now.saturating_duration_since(e.used) >= max_idle)
            .map(|(k, _)| *k)
            .collect();
        for key in old {
            self.take(key);
            self.evicted.insert(key);
        }
        if let Some(cold) = &mut self.cold {
            cold.expire(now, max_idle, &mut self.evicted);
        }
    }

    /// Column forgotten (unloaded): no base any more.
    pub fn forget(&mut self, key: Key) {
        self.take(key);
        self.first_live.remove(&key);
    }

    /// New world: no base any more.
    pub fn clear(&mut self) {
        self.hot.clear();
        self.probation.clear();
        self.protected.clear();
        (self.hot_bytes, self.protected_bytes, self.fluid_bytes) = (0, 0, 0);
        if let Some(cold) = &mut self.cold {
            cold.clear();
        }
        self.evicted.clear();
        self.first_live.clear();
    }

    /// Gauges of the perf log.
    fn report(&self) {
        crate::perf_log::mesh_cache(self.hot_bytes, self.hot.len());
        let cold = self.cold.as_ref().map_or(0, ColdStage::len);
        crate::perf_log::mesh_cache_shares(self.fluid_bytes, self.hot.len() + cold + self.evicted.len());
        crate::perf_log::mesh_cache_stages(self.cold.as_ref().map_or(0, |c| c.bytes), self.budget);
    }
}

/// Bytes of fluid faces in the flat mesh of `m` (`words` long): whatever is neither a face
/// (6 words) nor an index (lengths, starts, communications). The starts give the face counts,
/// and stay when the pool has already freed the faces and fluids (`finish`).
fn packed_fluid_bytes(m: &BlockMesh, words: usize) -> usize {
    let faces: usize = m.starts.iter().map(|s| s.last().map_or(0, |&n| n as usize)).sum();
    let index: usize =
        3 + 2 * m.vis.len() + (0..3).map(|l| 4 + m.starts[l].len() + m.fluid_starts[l].len()).sum::<usize>();
    words.saturating_sub(6 * faces + index) * 4
}

#[cfg(test)]
mod tests;
