//! Columns loaded but not meshed yet, as the visibility walk sees them. The game's walk crosses the
//! empty sections (air alone) of a loaded column it has not compiled, and holds its other sections
//! without going through them (only the camera's section opens on every side, whatever it holds).
//! Without them a column waiting for its mesh (every column right after a teleport or at loading,
//! the outermost ring of the loaded columns always) would stop the walk like an absent one, and a
//! camera in such a column would have no section to start from.

use hashbrown::HashMap;
use rc_mesh::visibility::{VIS_ALL, VIS_NONE, Vis};

use super::walk_inputs::Connectivity;

/// Provisional connectivity of a section of a column not meshed yet: open on every side when it
/// holds air alone, closed otherwise.
pub(super) fn provisional_connectivity(section: &rc_world::Section) -> Vis {
    if section.is_empty() { VIS_ALL } else { VIS_NONE }
}

/// Provisional connectivity of the columns loaded but not meshed yet.
#[derive(Default)]
pub(super) struct UnmeshedColumns {
    connectivity: HashMap<(i32, i32), Connectivity>,
}

impl UnmeshedColumns {
    /// Column `key` holds `sections`: its provisional connectivity, and whether it changed.
    pub fn note(&mut self, key: (i32, i32), sections: &[rc_world::Section]) -> bool {
        let provisional = || sections.iter().map(provisional_connectivity);
        if self.connectivity.get(&key).is_some_and(|old| old.iter().copied().eq(provisional())) {
            return false;
        }
        self.connectivity.insert(key, provisional().collect());
        true
    }

    pub fn get(&self, key: (i32, i32)) -> Option<&Connectivity> {
        self.connectivity.get(&key)
    }

    /// Column `key` is meshed or forgotten: its provisional connectivity, if it had one.
    pub fn remove(&mut self, key: (i32, i32)) -> Option<Connectivity> {
        self.connectivity.remove(&key)
    }

    pub fn clear(&mut self) {
        self.connectivity.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(i32, i32), &Connectivity)> {
        self.connectivity.iter()
    }

    /// The map itself, for the compiled sections (`compiled_sections.rs`).
    pub fn as_map(&self) -> &HashMap<(i32, i32), Connectivity> {
        &self.connectivity
    }
}

#[cfg(test)]
#[path = "unmeshed_columns_test.rs"]
mod tests;
