//! The relight of the end of the tick on its own thread (`RUSTCRAFT_OPT_LIGHT_ASYNC`, `=0`: on the
//! server thread as before).
//!
//! The game computes the light of a tick's changes on a background thread while its server
//! thread sleeps or reads packets; ours computed it at the end of the tick, inside the measured
//! tick. Here the lot leaves for the `light-main` thread at that same point, with a copy of
//! everything the engine reads (the blocks and light layers of the columns it may ask for,
//! [`rc_light::relight_columns`]) and of the filter of its `light_update`s (column present and
//! sent), both taken at the instant the old computation ran. The server thread goes on with its
//! loop; the lot is joined (waited for, then its layers, dirty marks and `light_update`s put in
//! place exactly as the old computation left them) before the first reader or writer of those
//! outputs: the head of the next tick, a packet, a save, a chunk sent from the copied columns,
//! generated or read chunks arriving, the loading screen, opening to LAN. Between the launch and
//! the join no one reads or writes them, and the lot's inputs are frozen: every observable result
//! (packets and their order, light read by the game, saves) is the old one. Launched only from the
//! real loop turn (`Server::run_turn`), with LAN closed and the blocks sent in the tick's
//! broadcast (`RUSTCRAFT_OPT_BLOCKS_SENT_AT_TICK_END` off): a test that drives `game_tick` itself
//! keeps the old computation.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use rc_world::{LightLayer, Section};

use crate::play::{RelightInput, Relit};
use crate::server::Server;

/// Shipped on; `RUSTCRAFT_OPT_LIGHT_ASYNC=0`: the relight on the server thread, as before.
pub(crate) fn switch_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_LIGHT_ASYNC")
}

/// Where a lot in flight is joined: the reader or writer that came first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Site {
    /// Head of the next tick (its relight, then every light read of the tick).
    Tick,
    /// First packet of the client (placements, commands and predicates read the light).
    Packets,
    /// Opening to LAN requested (guests get chunks and light at once).
    Lan,
    /// Save (pause, automatic, command): layers and dirty marks go to disk.
    Save,
    /// Closing (save, generator stopped, parked worlds).
    Close,
    /// First output of the generation thread (whole layers replaced, light sent at once).
    Generator,
    /// Chunks read put in place (caution: they join the map the copy was taken from).
    Adopt,
    /// A chunk sent whose light the lot writes.
    Send,
    /// Loading screen active (it loads the player's chunks).
    Prep,
    /// Guards: an entry that must never meet a lot in flight (tests: panic; otherwise joined).
    RelightFlush,
    Exchange,
    Broadcast,
}

/// One column of the copy.
#[derive(Default)]
struct Col {
    sections: Vec<Section>,
    sky: Vec<LightLayer>,
    block: Vec<LightLayer>,
}

/// The columns the relight may ask for, copied: the blocks, and the light layers (shared
/// `Arc`s: the engine copies a layer before its first write, the server's stays as it was).
#[derive(Default)]
pub(crate) struct Snapshot {
    cols: hashbrown::HashMap<(i32, i32), Col>,
    /// Emptied columns kept for the next lot (their buffers).
    spare: Vec<Col>,
    /// Columns the engine may ask for, sorted (tests: a request outside is a fault).
    #[cfg(test)]
    bound: Vec<(i32, i32)>,
    #[cfg(test)]
    missed: std::sync::atomic::AtomicBool,
}

impl Snapshot {
    #[cfg(test)]
    fn check(&self, cx: i32, cz: i32) {
        if self.bound.binary_search(&(cx, cz)).is_err() {
            self.missed.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl rc_light::LightGrid for Snapshot {
    fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])> {
        #[cfg(test)]
        self.check(cx, cz);
        let c = self.cols.get(&(cx, cz))?;
        Some((&c.sections, &c.sky, &c.block))
    }

    fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]> {
        #[cfg(test)]
        self.check(cx, cz);
        let c = self.cols.get_mut(&(cx, cz))?;
        Some(if sky { &mut c.sky } else { &mut c.block })
    }
}

/// The filter of the lot's `light_update`s as it stood at the launch, and the columns whose
/// layers the lot holds.
#[derive(Default)]
pub(crate) struct Frozen {
    /// Columns a `light_update` may concern, sorted: those the engine may write and their
    /// neighbours (edge sections are signalled next door), and the changes' columns at ±2 (layers
    /// created around a change and their neighbours).
    domain: Vec<(i32, i32)>,
    /// Those that were present and sent, sorted.
    sent: Vec<(i32, i32)>,
    /// Columns copied (present among those the engine may ask for), sorted: their light comes
    /// back with the lot.
    held: Vec<(i32, i32)>,
    /// Buffer of the columns the engine may ask for, kept between lots.
    bound: Vec<(i32, i32)>,
}

impl Frozen {
    /// The column gets the `light_update`, as decided at the launch.
    pub(crate) fn keeps(&self, x: i32, z: i32) -> bool {
        // By construction every column the lot signals is in the domain (see `domain`).
        #[cfg(test)]
        assert!(self.domain.binary_search(&(x, z)).is_ok(), "column ({x}, {z}) signalled outside the frozen filter");
        self.sent.binary_search(&(x, z)).is_ok()
    }
}

/// A lot, sent to the light thread and back.
struct Job {
    input: RelightInput,
    snap: Snapshot,
    tables: Arc<rc_light::LightTables>,
    relit: Option<Relit>,
    /// Tests (`LightAsync::hold`): the thread starts the lot only once the join asks for it.
    #[cfg(test)]
    hold: Option<Receiver<()>>,
}

/// The light thread: one lot at a time, like the game's serial light queue.
struct Worker {
    jobs: Sender<Box<Job>>,
    done: Receiver<Box<Job>>,
}

impl Worker {
    fn start() -> Option<Worker> {
        let (jobs, todo) = crossbeam_channel::unbounded::<Box<Job>>();
        let (finished, done) = crossbeam_channel::unbounded();
        // Outside any rayon pool: the engine's parallel parts go to its own `light-N` pool, as
        // they did from the server thread.
        std::thread::Builder::new()
            .name("light-main".into())
            .spawn(move || {
                while let Ok(mut job) = todo.recv() {
                    #[cfg(test)]
                    if let Some(hold) = job.hold.take() {
                        let _ = hold.recv();
                    }
                    let Job { input, snap, tables, relit, .. } = &mut *job;
                    *relit = Some(input.compute(snap, tables));
                    if finished.send(job).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Worker { jobs, done })
    }
}

/// The server's side (`Server::light_async`), outside the world swaps: a lot belongs to the
/// active world and is joined before any swap.
pub(crate) struct LightAsync {
    /// `RUSTCRAFT_OPT_LIGHT_ASYNC`, per server (the tests compare both paths in one process).
    pub(crate) on: bool,
    /// `game_tick` is running inside `Server::run_turn`.
    pub(crate) in_turn: bool,
    worker: Option<Worker>,
    /// Lot in flight: its frozen filter.
    flight: Option<Frozen>,
    /// Buffers of the last lot, for the next one.
    spare: Option<(Snapshot, Frozen)>,
    /// Tests: every lot waits for its join before starting, so each phase meets it unfinished.
    #[cfg(test)]
    pub(crate) hold: bool,
    #[cfg(test)]
    release: Option<Sender<()>>,
    /// Tests: where each lot was joined.
    #[cfg(test)]
    pub(crate) joins: Vec<Site>,
    /// Tests: fingerprint of the light each save was about to write (`Some`: recorded).
    #[cfg(test)]
    pub(crate) saves: Option<Vec<String>>,
}

impl Default for LightAsync {
    fn default() -> Self {
        LightAsync {
            on: switch_on(),
            in_turn: false,
            worker: None,
            flight: None,
            spare: None,
            #[cfg(test)]
            hold: false,
            #[cfg(test)]
            release: None,
            #[cfg(test)]
            joins: Vec::new(),
            #[cfg(test)]
            saves: None,
        }
    }
}

impl LightAsync {
    /// A lot is in flight.
    pub(crate) fn in_flight(&self) -> bool {
        self.flight.is_some()
    }

    /// The lot in flight computes the light of column `p` (a chunk packet of it must wait).
    pub(crate) fn holds(&self, p: (i32, i32)) -> bool {
        self.flight.as_ref().is_some_and(|f| f.held.binary_search(&p).is_ok())
    }
}

/// Measurement of the launch's copy (`bench_light_async`): launches, columns copied, nanoseconds.
pub(crate) mod stats {
    use std::sync::atomic::AtomicU64;

    pub(crate) static LAUNCHES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static COLUMNS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static COPY_NS: AtomicU64 = AtomicU64::new(0);
}

/// Tests: fingerprint of the light layers of the loaded chunks (`dirty_only`: those a save writes),
/// with their dirty marks.
#[cfg(test)]
pub(crate) fn light_digest(s: &Server, dirty_only: bool) -> String {
    let mut keys: Vec<_> = s
        .chunks
        .iter()
        .filter_map(|(k, c)| c.as_ref().map(|c| (*k, c)))
        .filter(|(_, c)| !dirty_only || c.dirty)
        .collect();
    keys.sort_by_key(|(k, _)| *k);
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for ((x, z), c) in keys {
        ctx.update(&x.to_le_bytes());
        ctx.update(&z.to_le_bytes());
        ctx.update(&[u8::from(c.dirty)]);
        for l in c.sky.iter().chain(&c.block) {
            match l {
                Some(l) => ctx.update(&l[..]),
                None => ctx.update(b"-"),
            }
        }
    }
    ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
thread_local! {
    /// Sabotage of the proofs (tests only): this join site is skipped.
    pub(crate) static SKIPPED_JOIN: std::cell::Cell<Option<Site>> = const { std::cell::Cell::new(None) };
}

impl Server {
    /// End of the tick (`game_tick`): the relight of the tick's changes, on the light thread
    /// when the lot may run there (see the module), otherwise right here as before.
    pub(crate) fn relight_changed_blocks_at_tick_end(&mut self) {
        if self.relight.is_empty() {
            return;
        }
        if !(self.light_async.on && self.light_async.in_turn && self.lan.is_none()) {
            self.relight_changed_blocks();
            return;
        }
        if self.light_async.worker.is_none() {
            self.light_async.worker = Worker::start();
        }
        if self.light_async.worker.is_none() {
            // Thread impossible to start: computed here.
            self.relight_changed_blocks();
            return;
        }
        self.light_guard(Site::RelightFlush);
        let relight = std::mem::take(&mut self.relight);
        let relight_sky = std::mem::take(&mut self.relight_sky);
        // On the server's chunks, as before: missing layers created, trace written.
        let input = self.relight_prepare(&relight, &relight_sky);
        let t0 = std::time::Instant::now();
        let (mut snap, mut frozen) = self.light_async.spare.take().unwrap_or_default();
        let mut bound = std::mem::take(&mut frozen.bound);
        bound.clear();
        rc_light::relight_columns(&input.local, input.sky_local.as_deref(), input.has_sky, &mut bound);
        frozen.held.clear();
        for &k in &bound {
            let Some(Some(c)) = self.chunks.get(&k) else { continue };
            let mut col = snap.spare.pop().unwrap_or_default();
            col.sections.clone_from(&c.sections);
            col.sky.clone_from(&c.sky);
            col.block.clone_from(&c.block);
            snap.cols.insert(k, col);
            frozen.held.push(k);
        }
        // `light_update` filter: present and sent now, over every column the lot may signal.
        let mut domain = std::mem::take(&mut frozen.domain);
        domain.clear();
        for &(x, z) in &bound {
            domain.extend((-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (x + dx, z + dz))));
        }
        for p in &input.local {
            let (x, z) = (p[0] >> 4, p[2] >> 4);
            domain.extend((-2..=2).flat_map(|dx| (-2..=2).map(move |dz| (x + dx, z + dz))));
        }
        domain.sort_unstable();
        domain.dedup();
        frozen.sent.clear();
        frozen.sent.extend(
            domain.iter().copied().filter(|k| matches!(self.chunks.get(k), Some(Some(_))) && self.sent.contains(k)),
        );
        frozen.domain = domain;
        #[cfg(test)]
        {
            snap.bound.clone_from(&bound);
            snap.missed = Default::default();
        }
        frozen.bound = bound;
        {
            use std::sync::atomic::Ordering::Relaxed;
            stats::LAUNCHES.fetch_add(1, Relaxed);
            stats::COLUMNS.fetch_add(frozen.held.len() as u64, Relaxed);
            stats::COPY_NS.fetch_add(t0.elapsed().as_nanos() as u64, Relaxed);
        }
        #[allow(unused_mut)]
        let mut job = Box::new(Job {
            input,
            snap,
            tables: self.light.clone(),
            relit: None,
            #[cfg(test)]
            hold: None,
        });
        #[cfg(test)]
        if self.light_async.hold {
            let (release, hold) = crossbeam_channel::bounded(1);
            job.hold = Some(hold);
            self.light_async.release = Some(release);
        }
        let Some(w) = &self.light_async.worker else { return };
        if let Err(back) = w.jobs.send(job) {
            // Thread gone: computed here, on the copy (same inputs).
            self.light_async.worker = None;
            let mut job = back.0;
            let Job { input, snap, tables, relit, .. } = &mut *job;
            *relit = Some(input.compute(snap, tables));
            self.light_async.flight = Some(frozen);
            self.finish_lot(job, Site::RelightFlush);
            return;
        }
        self.light_async.flight = Some(frozen);
    }

    /// The lot in flight, if any, waited for and put in place: before a reader or writer of the
    /// light, the dirty marks or the queued `light_update`s (see the module).
    pub(crate) fn join_light(&mut self, site: Site) {
        if self.light_async.flight.is_none() {
            return;
        }
        #[cfg(test)]
        if SKIPPED_JOIN.with(std::cell::Cell::get) == Some(site) {
            return;
        }
        #[cfg(test)]
        drop(self.light_async.release.take());
        let job = self.light_async.worker.as_ref().and_then(|w| w.done.recv().ok());
        let Some(job) = job else { panic!("light thread stopped with a lot in flight") };
        self.finish_lot(job, site);
    }

    /// A lot computed: its layers, dirty marks and `light_update`s in place, its buffers kept.
    fn finish_lot(&mut self, mut job: Box<Job>, site: Site) {
        let Some(frozen) = self.light_async.flight.take() else { return };
        #[cfg(test)]
        assert!(
            !job.snap.missed.load(std::sync::atomic::Ordering::Relaxed),
            "the relight asked for a column outside its copy"
        );
        #[cfg(not(test))]
        let _ = site;
        // The copied columns are still in the map (a column leaves it only with a swap of the
        // whole map, which joins first) and no one changed their light since the copy.
        for &k in &frozen.held {
            let (Some(Some(c)), Some(col)) = (self.chunks.get_mut(&k), job.snap.cols.get_mut(&k)) else { continue };
            std::mem::swap(&mut c.sky, &mut col.sky);
            std::mem::swap(&mut c.block, &mut col.block);
        }
        let relit = job.relit.take().expect("lot computed");
        self.relight_finish(&job.input, relit, Some(&frozen));
        // Buffers kept, emptied: no stale section or layer stays alive until the next lot.
        let mut snap = std::mem::take(&mut job.snap);
        for (_, mut col) in snap.cols.drain() {
            col.sections.clear();
            col.sky.clear();
            col.block.clear();
            snap.spare.push(col);
        }
        self.light_async.spare = Some((snap, frozen));
        #[cfg(test)]
        self.light_async.joins.push(site);
    }

    /// Tests (`LightAsync::saves`): what a save is about to write of the light.
    #[cfg(test)]
    pub(crate) fn note_light_saved(&mut self) {
        if self.light_async.saves.is_some() {
            let d = light_digest(self, true);
            self.light_async.saves.get_or_insert_default().push(d);
        }
    }

    /// Entry that no lot in flight may reach (a reader before it lacks its join): a fault in
    /// the tests, joined otherwise.
    pub(crate) fn light_guard(&mut self, site: Site) {
        if !self.light_async.in_flight() {
            return;
        }
        // Tests: a fault even under a skipped join (the sabotage is then caught here).
        if cfg!(test) {
            panic!("light lot in flight at {site:?}: a reader before it lacks its join");
        }
        self.join_light(site);
    }
}
