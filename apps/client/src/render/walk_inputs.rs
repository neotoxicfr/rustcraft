//! What the visibility walk reads of the loaded columns, kept up to date as columns come and go
//! instead of being remade from every column whenever one changes: the connectivity snapshot handed
//! to the walk task. (The walk's radius is the render distance, nothing to keep.)
//!
//! Switch (read once, on by default, `=0` = the old path): `RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT`.

use std::sync::Arc;

use hashbrown::HashMap;

use super::VisSnapshot;

/// Connectivity between the faces of each section of a column.
pub(super) type Connectivity = Arc<[rc_mesh::visibility::Vis]>;

/// `RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT`: the next snapshot patches the columns changed since the
/// last one instead of copying every column.
pub(super) fn incremental_vis_snapshot() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT")
}

/// Connectivity of every loaded column as handed to the walk tasks, and the columns whose
/// connectivity changed since it was made: on the incremental path the next snapshot patches these
/// few entries (in place when no walk task still reads it, otherwise on a copy) instead of copying
/// every column.
pub(super) struct VisSnapshotStore {
    /// Snapshot and the connectivity version (`vis_rev`) it shows.
    held: Option<(u64, VisSnapshot)>,
    /// Columns changed since `held` was made (incremental path).
    changed: Vec<(i32, i32)>,
    incremental: bool,
    #[cfg(test)]
    removals_ignored: bool,
}

impl VisSnapshotStore {
    pub fn new(incremental: bool) -> Self {
        VisSnapshotStore {
            held: None,
            changed: Vec::new(),
            incremental,
            #[cfg(test)]
            removals_ignored: false,
        }
    }

    /// The connectivity of column `key` changed: it appeared, left, or connects differently.
    pub fn changed(&mut self, key: (i32, i32)) {
        let Some(columns) = self.held.as_ref().filter(|_| self.incremental).map(|(_, s)| s.len()) else { return };
        // More changes than columns (a teleport): copying everything costs no more, and the list
        // stays bounded.
        if self.changed.len() >= columns {
            self.forget();
        } else {
            self.changed.push(key);
        }
    }

    /// Every column is gone.
    pub fn forget(&mut self) {
        self.held = None;
        self.changed.clear();
    }

    /// The snapshot held, if it shows connectivity version `vis_rev`.
    pub fn current(&self, vis_rev: u64) -> Option<VisSnapshot> {
        self.held.as_ref().filter(|(rev, _)| *rev == vis_rev).map(|(_, s)| s.clone())
    }

    /// Snapshot of connectivity version `vis_rev`: the held one patched with the columns changed
    /// since (`connectivity` of each, `None` once gone), otherwise made from every column (`all`).
    pub fn remake(
        &mut self,
        vis_rev: u64,
        connectivity: impl Fn((i32, i32)) -> Option<Connectivity>,
        all: impl FnOnce() -> HashMap<(i32, i32), Connectivity>,
    ) -> VisSnapshot {
        if self.incremental
            && let Some((rev, snapshot)) = &mut self.held
        {
            // Only this store holds the snapshot once the walk that read it finished: patched in
            // place; a walk still running keeps its own copy.
            let map = Arc::make_mut(snapshot);
            for key in self.changed.drain(..) {
                match connectivity(key) {
                    Some(v) => {
                        map.insert(key, v);
                    }
                    #[cfg(test)]
                    None if self.removals_ignored => {}
                    None => {
                        map.remove(&key);
                    }
                }
            }
            *rev = vis_rev;
            return snapshot.clone();
        }
        let snapshot: VisSnapshot = Arc::new(all());
        self.changed.clear();
        self.held = Some((vis_rev, snapshot.clone()));
        snapshot
    }
}

#[cfg(test)]
#[path = "walk_inputs_test.rs"]
mod tests;
