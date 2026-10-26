//! Saved chunks: reading outside the server thread and sending in slices.
//!
//! Two switches, read once (default 0 until the calm-machine scene bench has confirmed the
//! gain, teleportation included; `=1` enables):
//! - `RUSTCRAFT_OPT_CHUNKSTREAM`: the send queue is loaded and sent in slices under a time
//!   budget per tick, instead of being loaded entirely before the first send. The first
//!   slice always covers the spawn square (17 × 17 around the player); the rest waits for the next
//!   tick, without ever being marked sent.
//! - `RUSTCRAFT_OPT_ASYNC_LOAD`: the send queue is read by a separate thread (chunks, entities,
//!   points of interest), on the model of the generation thread. Finished batches are picked up
//!   at the start of the tick and adopted one by one, in request order, by the same code as the
//!   synchronous read. A position loaded in the meantime by another caller (synchronous read: it
//!   stays for the whole rest of the server) or out of view is dropped from the batch.
//!
//! Settings of these two paths (no effect on the old path, default 1):
//! `RUSTCRAFT_OPT_CHUNKSPLIT` (reads split into tasks of 4 chunks per region) and
//! `RUSTCRAFT_OPT_ASYNC_LATE` (second pickup just before the send of the tick),
//! `RUSTCRAFT_OPT_ASYNC_NEARSYNC` (both paths together: spawn square read immediately);
//! tick budget:
//! `RUSTCRAFT_CHUNKSTREAM_MS`.
//!
//! The official game also loads its chunks asynchronously: a chunk that arrives one or
//! several ticks later is neither perceptible nor detectable by a server; the set of
//! chunks sent, their content and the saved state do not change (`tests_opt`, module
//! `loading`).

use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};
use hashbrown::HashSet;
use rc_data::VersionData;
use rc_proto::nbt::Nbt;

use crate::states::States;
use crate::world::{Dimension, Load};

/// Half-side of the square always processed in the first slice of a send (17 × 17).
pub(crate) const SPAWN_RADIUS: i32 = 8;
/// Following slices: number of chunks loaded (or requested) then sent at once.
pub(crate) const SLICE: usize = 32;

/// Budget of a sliced send (ms per tick, `RUSTCRAFT_CHUNKSTREAM_MS`, default 10).
pub(crate) fn budget() -> std::time::Duration {
    static C: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ms =
        *C.get_or_init(|| rc_data::env_str!("RUSTCRAFT_CHUNKSTREAM_MS").and_then(|v| v.parse().ok()).unwrap_or(10));
    std::time::Duration::from_millis(ms)
}

#[cfg(test)]
thread_local! {
    /// Tests: (sliced send, separate read) forced on this thread.
    pub(crate) static TEST_OPT: std::cell::Cell<Option<(bool, bool)>> = const { std::cell::Cell::new(None) };
    /// Tests: deliberate sabotage (0 none); see `tests_opt::loading`.
    pub(crate) static SABOTAGE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    /// Tests: positions adopted, in order (synchronous path as well as picked-up batches).
    pub(crate) static ADOPTED: std::cell::RefCell<Vec<(i32, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn sabotage() -> u8 {
    SABOTAGE.with(std::cell::Cell::get)
}

/// Sliced send under budget (`RUSTCRAFT_OPT_CHUNKSTREAM`).
pub(crate) fn stream_on() -> bool {
    #[cfg(test)]
    if let Some((s, _)) = TEST_OPT.with(std::cell::Cell::get) {
        return s;
    }
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_CHUNKSTREAM", false)
}

/// Reading of the send queue on a separate thread (`RUSTCRAFT_OPT_ASYNC_LOAD`).
pub(crate) fn async_on() -> bool {
    #[cfg(test)]
    if let Some((_, a)) = TEST_OPT.with(std::cell::Cell::get) {
        return a;
    }
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_ASYNC_LOAD", false)
}

/// Sliced send AND separate read: the first slice (spawn square) is read immediately
/// on the server thread, the rest on the read thread (`RUSTCRAFT_OPT_ASYNC_NEARSYNC`,
/// default 1). Without it, the first chunks wait for the end of the read and the next tick.
pub(crate) fn near_sync() -> bool {
    #[cfg(test)]
    if let Some(v) = TEST_NEAR.with(std::cell::Cell::get) {
        return v;
    }
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_ASYNC_NEARSYNC", true)
}

#[cfg(test)]
thread_local! {
    /// Tests: spawn square read immediately, forced on this thread.
    pub(crate) static TEST_NEAR: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Second pickup of the read thread, just before the chunks of the tick are sent
/// (`RUSTCRAFT_OPT_ASYNC_LATE`, default 1; only acts with the separate read). Without it, a batch finished
/// during the tick waits for the start of the next (up to 50 ms more before its send).
pub(crate) fn late_poll() -> bool {
    #[cfg(test)]
    if let Some(v) = TEST_LATE.with(std::cell::Cell::get) {
        return v;
    }
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_ASYNC_LATE", true)
}

#[cfg(test)]
thread_local! {
    /// Tests: second pickup forced on this thread.
    pub(crate) static TEST_LATE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Reads of the two new paths split into tasks of at most 4 chunks
/// (`RUSTCRAFT_OPT_CHUNKSPLIT`, default 1; `0`: one task per region like the old path).
/// A slice of 32 neighbouring chunks fits in one or two regions: without splitting, it is
/// read on one or two threads.
pub(crate) fn per_task() -> usize {
    #[cfg(test)]
    if let Some(v) = TEST_SPLIT.with(std::cell::Cell::get) {
        return if v { PER_TASK } else { usize::MAX };
    }
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_CHUNKSPLIT", true) { PER_TASK } else { usize::MAX }
}

const PER_TASK: usize = 4;

#[cfg(test)]
thread_local! {
    /// Tests: splitting of reads forced on this thread.
    pub(crate) static TEST_SPLIT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Everything read from disk for a list of chunks, before adopting it.
pub(crate) struct Read {
    pub(crate) chunks: Vec<((i32, i32), Load)>,
    pub(crate) entities: Vec<((i32, i32), Option<Nbt>)>,
    pub(crate) poi: Vec<((i32, i32), Option<Nbt>)>,
}

impl Read {
    /// Reading of the three region folders (no server state touched), tasks of at most
    /// `per_task` chunks per region.
    pub(crate) fn of(
        dim: &Dimension,
        list: &[(i32, i32)],
        data: &VersionData,
        states: &States,
        per_task: usize,
    ) -> Read {
        Read {
            chunks: dim.load_split(list, data, states, per_task),
            entities: dim.load_raw_split("entities", list, per_task),
            poi: dim.load_raw_split("poi", list, per_task),
        }
    }

    /// Keeps only the positions of `keep`.
    pub(crate) fn retain(&mut self, keep: &HashSet<(i32, i32)>) {
        self.chunks.retain(|(p, _)| keep.contains(p));
        self.entities.retain(|(p, _)| keep.contains(p));
        self.poi.retain(|(p, _)| keep.contains(p));
    }
}

/// Batch read by the thread: dimension, positions in request order, contents.
pub(crate) struct Batch {
    pub(crate) dim: String,
    pub(crate) list: Vec<(i32, i32)>,
    pub(crate) read: Read,
}

struct Job {
    dim: Dimension,
    list: Vec<(i32, i32)>,
    per_task: usize,
}

/// Server side of the read thread.
pub(crate) struct Loader {
    tx: Option<Sender<Job>>,
    rx: Receiver<Batch>,
    /// Positions requested not yet adopted (dimension `dim`).
    pub(crate) pending: HashSet<(i32, i32)>,
    pub(crate) dim: String,
    thread: Option<JoinHandle<()>>,
}

impl Loader {
    pub(crate) fn start(data: Arc<VersionData>, states: Arc<States>) -> Option<Loader> {
        let (tx, rx_in) = crossbeam_channel::unbounded::<Job>();
        let (tx_out, rx) = crossbeam_channel::unbounded::<Batch>();
        let thread = std::thread::Builder::new()
            .name("solo-read".into())
            .spawn(move || {
                while let Ok(j) = rx_in.recv() {
                    let read = Read::of(&j.dim, &j.list, &data, &states, j.per_task);
                    if tx_out.send(Batch { dim: j.dim.name, list: j.list, read }).is_err() {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Loader { tx: Some(tx), rx, pending: HashSet::new(), dim: String::new(), thread: Some(thread) })
    }

    /// Requests the reading of `list` (priority order); positions already waiting are
    /// skipped. A request for another dimension forgets the wait of the previous one (its batches
    /// will be dropped on arrival).
    pub(crate) fn want(&mut self, dim: &Dimension, list: &[(i32, i32)]) {
        if self.dim != dim.name {
            self.pending.clear();
            self.dim.clone_from(&dim.name);
        }
        let list: Vec<(i32, i32)> = list.iter().copied().filter(|p| self.pending.insert(*p)).collect();
        if list.is_empty() {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(Job { dim: dim.clone(), list, per_task: per_task() });
        }
    }

    /// Finished batches, in request order (without waiting).
    pub(crate) fn ready(&self) -> Vec<Batch> {
        self.rx.try_iter().collect()
    }

    /// Tests: finished batches not yet picked up.
    #[cfg(test)]
    pub(crate) fn ready_len(&self) -> usize {
        self.rx.len()
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
