//! What the visibility walk reads of the loaded columns, kept up to date as columns come and go
//! instead of being remade from every column whenever one changes: the farthest loaded column (the
//! walk's radius) and the connectivity snapshot handed to the walk task.
//!
//! Switches (read once, on by default, `=0` = the old path): `RUSTCRAFT_OPT_INCREMENTAL_VIEW_RADIUS`,
//! `RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT`.

use std::sync::Arc;

use hashbrown::HashMap;

use super::VisSnapshot;

/// Connectivity between the faces of each section of a column.
pub(super) type Connectivity = Arc<[rc_mesh::visibility::Vis]>;

/// `RUSTCRAFT_OPT_INCREMENTAL_VIEW_RADIUS`: the farthest column is kept as columns are added, and only
/// measured again over every column when the farthest one leaves or the camera changes column.
pub(super) fn incremental_view_radius() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_INCREMENTAL_VIEW_RADIUS")
}

/// `RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT`: the next snapshot patches the columns changed since the
/// last one instead of copying every column.
pub(super) fn incremental_vis_snapshot() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_INCREMENTAL_VIS_SNAPSHOT")
}

/// Chebyshev distance between two columns.
fn column_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

/// Distance of the farthest of `keys` from `camera_column`, 0 without any.
pub(super) fn farthest_distance(camera_column: (i32, i32), keys: impl Iterator<Item = (i32, i32)>) -> i32 {
    keys.map(|k| column_distance(k, camera_column)).max().unwrap_or(0)
}

/// Farthest loaded column from the camera's column, kept as columns are added; the removal of the
/// farthest one, or a new camera column, only marks it to be measured again.
#[derive(Default)]
pub(super) struct FarthestColumn {
    /// Camera column it is measured from, and the distance; `None`: to measure again.
    held: Option<((i32, i32), i32)>,
    #[cfg(test)]
    removals_ignored: bool,
}

impl FarthestColumn {
    /// Column `key` was not loaded and now is.
    pub fn added(&mut self, key: (i32, i32)) {
        if let Some((camera, distance)) = &mut self.held {
            *distance = (*distance).max(column_distance(key, *camera));
        }
    }

    /// Column `key` was loaded and no longer is.
    pub fn removed(&mut self, key: (i32, i32)) {
        #[cfg(test)]
        if self.removals_ignored {
            return;
        }
        if self.held.is_some_and(|(camera, distance)| column_distance(key, camera) >= distance) {
            self.held = None;
        }
    }

    /// Every column is gone.
    pub fn forget(&mut self) {
        self.held = None;
    }

    /// Distance of the farthest loaded column from `camera_column`; `loaded` lists them when it must
    /// be measured again.
    pub fn distance<I: Iterator<Item = (i32, i32)>>(&mut self, camera_column: (i32, i32), loaded: impl FnOnce() -> I) -> i32 {
        match self.held {
            Some((camera, distance)) if camera == camera_column => distance,
            _ => {
                let distance = farthest_distance(camera_column, loaded());
                self.held = Some((camera_column, distance));
                distance
            }
        }
    }
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
