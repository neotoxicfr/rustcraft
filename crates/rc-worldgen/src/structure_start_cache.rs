//! Cache of the structure starts per chunk (see `structure.rs`).

use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::ThreadId;

use hashbrown::HashMap;

use super::Starts;

/// Tables of a sharded cache (a power of two).
const SHARDED_TABLES: usize = 64;
/// Entries the whole cache holds before its tables are emptied.
const CAPACITY: usize = 200_000;

/// One table of the cache.
#[derive(Default)]
pub(super) struct Table {
    /// Start lists computed, per chunk.
    pub(super) ready: HashMap<(i32, i32), Starts>,
    /// Lists being computed, each by one thread (`RUSTCRAFT_OPT_STARTS_COMPUTED_ONCE`).
    pub(super) in_progress: HashMap<(i32, i32), Arc<Pending>>,
}

/// A start list that one thread is computing and the others wait for.
#[derive(Default)]
pub(super) struct Pending {
    pub(super) list: OnceLock<Starts>,
    computing_thread: Mutex<Option<ThreadId>>,
}

impl Pending {
    /// Thread that computes the list, once it has begun.
    pub(super) fn computing_thread(&self) -> Option<ThreadId> {
        *self.computing_thread.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn set_computing_thread(&self, thread: ThreadId) {
        *self.computing_thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread);
    }
}

/// One locked table, alone on its cache lines: a thread taking this lock does not slow down the
/// threads working on the neighbouring tables.
#[repr(align(128))]
#[derive(Default)]
struct LockedTable(Mutex<Table>);

/// Start lists already computed, per chunk: one table behind one lock, or 64 tables each behind
/// its own (`RUSTCRAFT_OPT_SHARDED_STARTS_CACHE`). The threads that walk the references of
/// neighbouring chunks all read the same few dozen chunks; on a single lock they wait for one
/// another (Nether: 20 to 36 waits per walk measured from 4 threads up).
pub(super) struct StartCache {
    tables: Box<[LockedTable]>,
    /// Entries a table holds before it is emptied. Lowered by the tests.
    pub(super) table_capacity: usize,
}

impl StartCache {
    pub(super) fn new(sharded: bool) -> Self {
        let tables = if sharded { SHARDED_TABLES } else { 1 };
        StartCache { tables: (0..tables).map(|_| LockedTable::default()).collect(), table_capacity: CAPACITY / tables }
    }

    /// Table of the chunk. The low bits of its position pick it, so that the chunks of one
    /// reference window (17 × 17) spread evenly over the tables.
    pub(super) fn table_of(&self, cx: i32, cz: i32) -> &Mutex<Table> {
        let low_bits = (cx & 7) as usize | ((cz & 7) as usize) << 3;
        &self.tables[low_bits & (self.tables.len() - 1)].0
    }

    /// Empties every table (tests: what reaching the capacity does, table by table).
    #[cfg(test)]
    pub(crate) fn clear(&self) {
        for table in self.tables.iter() {
            table.0.lock().unwrap_or_else(PoisonError::into_inner).ready.clear();
        }
    }

    /// Chunks held, sorted.
    #[cfg(test)]
    pub(crate) fn chunks(&self) -> Vec<(i32, i32)> {
        let mut chunks: Vec<(i32, i32)> = self
            .tables
            .iter()
            .flat_map(|t| t.0.lock().unwrap_or_else(PoisonError::into_inner).ready.keys().copied().collect::<Vec<_>>())
            .collect();
        chunks.sort_unstable();
        chunks
    }
}
