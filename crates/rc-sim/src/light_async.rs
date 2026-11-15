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
//!
//! The copy (`RUSTCRAFT_OPT_LIGHT_COPY_REUSE`, on; `=0`: as before) goes into storage kept from the
//! last lot: sections into their containers, light layers into sheets of the copy's own that the
//! engine writes in place. Sharing the server's layers instead made the engine copy each layer it
//! wrote, one allocation per written layer and per lot; at the join only the layers the lot changed
//! are copied back into the server's, which keep their light while the lot runs.

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

/// Shipped on; `RUSTCRAFT_OPT_LIGHT_COPY_REUSE=0`: the copy as before, the sections cloned in full
/// at each launch and the light layers shared with the server's (the engine then copies each layer
/// it writes, and the join swaps them in).
pub(crate) fn reuse_switch_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_LIGHT_COPY_REUSE")
}

type Sheet = Arc<[u8; rc_world::LIGHT_BYTES]>;

/// One column of the copy.
#[derive(Default)]
struct Col {
    sections: Vec<Section>,
    /// The layers the engine reads and writes. Reuse on: sheets of the copy's own, never shared,
    /// refilled from `sky_src` / `block_src` lot after lot, so the engine's writes (`Arc::make_mut`)
    /// copy nothing. Reuse off: the server's, shared.
    sky: Vec<LightLayer>,
    block: Vec<LightLayer>,
    /// Reuse on: the server's layers, shared from the launch until the light thread has copied
    /// them and found what the lot wrote (then dropped, so the server's are unique again at the join).
    sky_src: Vec<LightLayer>,
    block_src: Vec<LightLayer>,
}

/// The columns the relight may ask for, copied: the blocks, and the light layers (see [`Col`]).
#[derive(Default)]
pub(crate) struct Snapshot {
    /// `RUSTCRAFT_OPT_LIGHT_COPY_REUSE`, as it stood at the launch.
    reuse: bool,
    /// The last lot's columns, kept with their sections (reuse on: and their layers).
    cols: hashbrown::HashMap<(i32, i32), Col>,
    /// Columns out of the copy, kept for the next lot (their buffers).
    spare: Vec<Col>,
    /// Reuse on: sheets no column uses now, for the next column that has a layer there.
    free: Vec<Sheet>,
    /// Reuse on: the layers the lot changed (column, sky, position), copied back at the join.
    written: Vec<((i32, i32), bool, usize)>,
    /// Tests: the address of every sheet before the engine runs (one moved: the engine copied it).
    #[cfg(test)]
    sheets: Vec<usize>,
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

    /// The lot on the copy (light thread, or the server's if that thread is gone). Reuse on: the
    /// server's layers copied into the copy's own sheets first, a memcpy into storage kept from the
    /// last lot; then the engine, which writes those sheets in place as it writes the server's on
    /// the old path; then the layers it changed noted and the server's released.
    fn relight(&mut self, input: &RelightInput, tables: &rc_light::LightTables) -> Relit {
        if !self.reuse {
            return uncounted(|| input.compute(self, tables));
        }
        #[cfg(test)]
        let t0 = std::time::Instant::now();
        let Snapshot { cols, free, .. } = &mut *self;
        for col in cols.values_mut() {
            fill(&mut col.sky, &col.sky_src, free);
            fill(&mut col.block, &col.block_src, free);
        }
        #[cfg(test)]
        let fill_ns = t0.elapsed().as_nanos() as u64;
        #[cfg(test)]
        self.note_sheets();
        let relit = uncounted(|| input.compute(self, tables));
        #[cfg(test)]
        self.count_moved_sheets();
        #[cfg(test)]
        let t1 = std::time::Instant::now();
        let Snapshot { cols, written, .. } = &mut *self;
        written.clear();
        for (&k, col) in cols.iter_mut() {
            for (sky, own, src) in [(true, &col.sky, &mut col.sky_src), (false, &col.block, &mut col.block_src)] {
                for (i, pair) in own.iter().zip(src.iter()).enumerate() {
                    if matches!(pair, (Some(o), Some(s)) if **o != **s) {
                        written.push((k, sky, i));
                    }
                }
                src.clear();
            }
        }
        #[cfg(test)]
        stats::FILL_NS.fetch_add(fill_ns + t1.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        relit
    }

    /// Tests: every sheet's address, before the engine runs.
    #[cfg(test)]
    fn note_sheets(&mut self) {
        let Snapshot { cols, sheets, .. } = self;
        sheets.clear();
        for col in cols.values() {
            sheets.extend(col.sky.iter().chain(&col.block).map(|l| l.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize)));
        }
    }

    /// Tests: each sheet the engine copied (`Arc::make_mut` on a shared one) counts as one
    /// allocation of the lot's own (the maps are not changed in between: same order).
    #[cfg(test)]
    fn count_moved_sheets(&self) {
        let now = self.cols.values().flat_map(|col| col.sky.iter().chain(&col.block));
        let moved = now.zip(&self.sheets).filter(|&(l, &a)| l.as_ref().map_or(0, |l| Arc::as_ptr(l) as usize) != a).count();
        crate::light_async_tests::count_own(moved as u64);
    }
}

/// `own` takes the values of `src`, in the sheets it has (or free ones): once the copy's
/// sheets have grown, no allocation.
fn fill(own: &mut Vec<LightLayer>, src: &[LightLayer], free: &mut Vec<Sheet>) {
    let keep = own.len().min(src.len());
    free.extend(own.drain(keep..).flatten());
    own.resize(src.len(), None);
    for (o, s) in own.iter_mut().zip(src) {
        match s {
            None => free.extend(o.take()),
            Some(s) => {
                let sheet = o.get_or_insert_with(|| free.pop().unwrap_or_else(|| Arc::new([0; rc_world::LIGHT_BYTES])));
                // Never shared (the copy hands out no sheet): written in place.
                Arc::make_mut(sheet).copy_from_slice(&s[..]);
            }
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
    /// Buffers of `relight_columns`, kept between lots.
    columns: rc_light::ColumnsScratch,
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
    /// Tests: the light thread's own allocations so far (its sends back, up to the last lot's).
    #[cfg(test)]
    light_own: u64,
}

/// Work the old path does too (preparation, finish, the engine): left out of the count of the lots'
/// own allocations (tests, `a_lot_allocates_nothing_once_its_buffers_have_grown`).
fn uncounted<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(test)]
    let f = || crate::light_async_tests::left_out(f);
    f()
}

/// The light thread: one lot at a time, like the game's serial light queue.
struct Worker {
    jobs: Sender<Box<Job>>,
    done: Receiver<Box<Job>>,
}

impl Worker {
    fn start() -> Option<Worker> {
        // One lot at a time (launched once the last one is joined), so neither channel ever holds
        // more than one: bounded ones have their slot from the start, where an unbounded one
        // allocates a block every 31 messages.
        let (jobs, todo) = crossbeam_channel::bounded::<Box<Job>>(1);
        let (finished, done) = crossbeam_channel::bounded(1);
        // Outside any rayon pool: the engine's parallel parts go to its own `light-N` pool, as
        // they did from the server thread.
        std::thread::Builder::new()
            .name("light-main".into())
            .spawn(move || {
                while let Ok(mut job) = todo.recv() {
                    #[cfg(test)]
                    {
                        job.light_own = crate::light_async_tests::own_allocations();
                    }
                    #[cfg(test)]
                    if let Some(hold) = job.hold.take() {
                        let _ = hold.recv();
                    }
                    let sent = {
                        #[cfg(test)]
                        let _own = crate::light_async_tests::Own::start();
                        let Job { input, snap, tables, relit, .. } = &mut *job;
                        *relit = Some(snap.relight(input, tables));
                        finished.send(job)
                    };
                    if sent.is_err() {
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
    /// `RUSTCRAFT_OPT_LIGHT_COPY_REUSE`, per server likewise.
    pub(crate) reuse: bool,
    /// `game_tick` is running inside `Server::run_turn`.
    pub(crate) in_turn: bool,
    worker: Option<Worker>,
    /// Lot in flight: its frozen filter.
    flight: Option<Frozen>,
    /// The last lot's box and buffers, for the next one.
    spare: Option<(Box<Job>, Frozen)>,
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
    /// Tests: the light thread's own allocations, as the last lot joined reported them.
    #[cfg(test)]
    pub(crate) light_own: u64,
    /// Tests: layers the joined lots wrote (reuse on).
    #[cfg(test)]
    pub(crate) layers_written: u64,
}

impl Default for LightAsync {
    fn default() -> Self {
        LightAsync {
            on: switch_on(),
            reuse: reuse_switch_on(),
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
            #[cfg(test)]
            light_own: 0,
            #[cfg(test)]
            layers_written: 0,
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
    /// Tests: the part of the copy spent cloning the sections, nanoseconds.
    #[cfg(test)]
    pub(crate) static SECTIONS_NS: AtomicU64 = AtomicU64::new(0);
    /// Tests: layers the lots wrote (reuse on: copied back into the server's at the join; off:
    /// each copied once by the engine on the light thread, the server's old one freed at the join).
    #[cfg(test)]
    pub(crate) static LAYERS_WRITTEN: AtomicU64 = AtomicU64::new(0);
    /// Tests: the joins' part on the copy's layers (reuse on: the written ones copied back; off:
    /// the copy's layers emptied, the old ones freed), nanoseconds.
    #[cfg(test)]
    pub(crate) static BACK_NS: AtomicU64 = AtomicU64::new(0);
    /// Tests: reuse on, the light thread's copy of the layers into the copy's own sheets and its
    /// search for those written, nanoseconds.
    #[cfg(test)]
    pub(crate) static FILL_NS: AtomicU64 = AtomicU64::new(0);
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
        #[cfg(test)]
        let _own = crate::light_async_tests::Own::start();
        let relight = std::mem::take(&mut self.relight);
        let relight_sky = std::mem::take(&mut self.relight_sky);
        // On the server's chunks, as before: missing layers created, trace written.
        let input = uncounted(|| self.relight_prepare(&relight, &relight_sky));
        let t0 = std::time::Instant::now();
        // The last lot's box and buffers, refilled: once they have grown, a lot allocates nothing
        // of its own on this thread (but for a section whose palette or width changed).
        let (mut job, mut frozen) = match self.light_async.spare.take() {
            Some((mut job, frozen)) => {
                job.input = input;
                job.tables.clone_from(&self.light);
                (job, frozen)
            }
            None => {
                let job = Box::new(Job {
                    input,
                    snap: Snapshot::default(),
                    tables: self.light.clone(),
                    relit: None,
                    #[cfg(test)]
                    hold: None,
                    #[cfg(test)]
                    light_own: 0,
                });
                (job, Frozen::default())
            }
        };
        let Job { input, snap, .. } = &mut *job;
        let mut bound = std::mem::take(&mut frozen.bound);
        bound.clear();
        rc_light::relight_columns(
            &input.local,
            input.sky_local.as_deref(),
            input.has_sky,
            &mut frozen.columns,
            &mut bound,
        );
        frozen.held.clear();
        // A column copied by the last lot gets its own entry back: reuse on, its sections, kept,
        // take the new ones in their storage (`Section::clone_from`), and only a section whose
        // palette or width changed allocates; the server's layers are only shared here, the light
        // thread copies them into the column's own sheets (see `Snapshot::relight`).
        let reuse = self.light_async.reuse;
        snap.reuse = reuse;
        let Snapshot { cols, spare, .. } = &mut *snap;
        for &k in &bound {
            let Some(Some(c)) = self.chunks.get(&k) else { continue };
            let col = cols.entry(k).or_insert_with(|| spare.pop().unwrap_or_default());
            #[cfg(test)]
            let ts = std::time::Instant::now();
            if reuse {
                col.sections.clone_from(&c.sections);
            } else {
                col.sections = c.sections.clone();
            }
            #[cfg(test)]
            stats::SECTIONS_NS.fetch_add(ts.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            if reuse {
                col.sky_src.clone_from(&c.sky);
                col.block_src.clone_from(&c.block);
            } else {
                col.sky.clone_from(&c.sky);
                col.block.clone_from(&c.block);
            }
            frozen.held.push(k);
        }
        // The others leave the copy (the engine must find them absent), their buffers kept.
        let held = &frozen.held;
        cols.retain(|k, col| {
            let keep = held.binary_search(k).is_ok();
            if !keep {
                spare.push(std::mem::take(col));
            }
            keep
        });
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
        #[cfg(test)]
        {
            job.hold = None;
            if self.light_async.hold {
                let (release, hold) = crossbeam_channel::bounded(1);
                job.hold = Some(hold);
                self.light_async.release = Some(release);
            }
        }
        let Some(w) = &self.light_async.worker else { return };
        if let Err(back) = w.jobs.send(job) {
            // Thread gone: computed here, on the copy (same inputs).
            self.light_async.worker = None;
            let mut job = back.0;
            let Job { input, snap, tables, relit, .. } = &mut *job;
            *relit = Some(snap.relight(input, tables));
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
        let _own = crate::light_async_tests::Own::start();
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
        #[cfg(test)]
        let tb = std::time::Instant::now();
        if job.snap.reuse {
            // The layers the lot changed, copied into the server's: unique again (the light thread
            // dropped its share), so written in place, as the engine writes them on the old path. One
            // still shared elsewhere (a uniform sheet the preparation put there) is copied first, as
            // the engine's first write copies it on the old path.
            let snap = &job.snap;
            for &(k, sky, i) in &snap.written {
                let (Some(Some(c)), Some(col)) = (self.chunks.get_mut(&k), snap.cols.get(&k)) else { continue };
                let (to, from) = if sky { (&mut c.sky, &col.sky) } else { (&mut c.block, &col.block) };
                let (Some(Some(to)), Some(Some(from))) = (to.get_mut(i), from.get(i)) else { continue };
                if Arc::get_mut(to).is_none() {
                    uncounted(|| {
                        Arc::make_mut(to);
                    });
                }
                Arc::make_mut(to).copy_from_slice(&from[..]);
            }
            #[cfg(test)]
            {
                stats::LAYERS_WRITTEN.fetch_add(snap.written.len() as u64, std::sync::atomic::Ordering::Relaxed);
                self.light_async.layers_written += snap.written.len() as u64;
            }
        } else {
            for &k in &frozen.held {
                let (Some(Some(c)), Some(col)) = (self.chunks.get_mut(&k), job.snap.cols.get_mut(&k)) else { continue };
                #[cfg(test)]
                {
                    let pairs = c.sky.iter().zip(&col.sky).chain(c.block.iter().zip(&col.block));
                    let written =
                        pairs.filter(|(a, b)| matches!((a, b), (Some(a), Some(b)) if !Arc::ptr_eq(a, b))).count();
                    stats::LAYERS_WRITTEN.fetch_add(written as u64, std::sync::atomic::Ordering::Relaxed);
                }
                std::mem::swap(&mut c.sky, &mut col.sky);
                std::mem::swap(&mut c.block, &mut col.block);
            }
        }
        #[cfg(test)]
        let mut back_ns = tb.elapsed().as_nanos() as u64;
        let relit = job.relit.take().expect("lot computed");
        uncounted(|| self.relight_finish(&job.input, relit, Some(&frozen)));
        if !job.snap.reuse {
            // Layers emptied: one kept alive until the next lot would be shared with the server's,
            // and its next write copied.
            #[cfg(test)]
            let te = std::time::Instant::now();
            for col in job.snap.cols.values_mut() {
                col.sky.clear();
                col.block.clear();
            }
            #[cfg(test)]
            {
                back_ns += te.elapsed().as_nanos() as u64;
            }
        }
        #[cfg(test)]
        stats::BACK_NS.fetch_add(back_ns, std::sync::atomic::Ordering::Relaxed);
        #[cfg(test)]
        {
            self.light_async.light_own = job.light_own;
            uncounted(|| self.light_async.joins.push(site));
        }
        self.light_async.spare = Some((job, frozen));
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
