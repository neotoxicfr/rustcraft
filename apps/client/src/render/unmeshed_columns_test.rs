//! Proofs of the provisional connectivity of the columns waiting for their mesh
//! (`unmeshed_columns.rs`), and of the walk over it through the renderer's own lookup.

use hashbrown::HashMap;
use rc_mesh::visibility::{Graph, VIS_ALL, VIS_NONE};
use rc_world::{Container, Section};

use super::*;
use crate::render::column_upload::{ColumnGpu, walk_connectivity};

fn air() -> Section {
    Section::empty(0, 0)
}

fn stone() -> Section {
    Section { non_empty: 4096, blocks: Container::Single(1), biomes: Container::Single(0) }
}

/// Open on every side exactly when the section holds air alone; noted again, a column reports a
/// change only when the emptiness of one of its sections changed.
#[test]
fn provisional_connectivity_opens_only_the_empty_sections() {
    let mut unmeshed = UnmeshedColumns::default();
    assert!(unmeshed.note((3, -2), &[stone(), air(), air()]));
    assert_eq!(&**unmeshed.get((3, -2)).unwrap(), &[VIS_NONE, VIS_ALL, VIS_ALL]);
    assert!(!unmeshed.note((3, -2), &[stone(), air(), air()]), "same emptiness: no change");
    assert!(unmeshed.note((3, -2), &[stone(), stone(), air()]), "a section filled");
    assert_eq!(&**unmeshed.get((3, -2)).unwrap(), &[VIS_NONE, VIS_NONE, VIS_ALL]);
    assert!(unmeshed.remove((3, -2)).is_some() && unmeshed.get((3, -2)).is_none());
}

/// Three columns in a row along +x, four sections high: (0, 0) and (2, 0) meshed and open
/// everywhere, (1, 0) loaded but waiting for its mesh, stone in its two lower sections and air above.
fn row_with_a_column_waiting_for_its_mesh() -> (HashMap<(i32, i32), Connectivity>, UnmeshedColumns) {
    let open: Connectivity = [VIS_ALL; 4].into();
    let meshed = [((0, 0), open.clone()), ((2, 0), open)].into_iter().collect();
    let mut unmeshed = UnmeshedColumns::default();
    unmeshed.note((1, 0), &[stone(), stone(), air(), air()]);
    (meshed, unmeshed)
}

/// The walk reads the column waiting for its mesh like the game's: it crosses its air, holds its
/// stone sections without going through them, and starts from it when the camera is there. Before,
/// that column counted as absent: nothing beyond it, and nothing at all from a camera inside it.
#[test]
fn walk_crosses_the_empty_sections_of_a_column_waiting_for_its_mesh() {
    let (meshed_without_faces, unmeshed) = row_with_a_column_waiting_for_its_mesh();
    let no_mesh: HashMap<(i32, i32), ColumnGpu> = HashMap::new();
    let column = |x, z| walk_connectivity(&no_mesh, &meshed_without_faces, &unmeshed, (x, z)).map(|v| &v[..]);
    let mut graph = Graph::default();
    // Camera at the bottom of column (0, 0).
    let mut reached = graph.run(column, [8.5, 8.0, 8.5], 0, 4, 4, true).to_vec();
    reached.sort_unstable();
    assert!(reached.contains(&(1, 0, 0)), "a stone section is held: {reached:?}");
    assert!(reached.contains(&(1, 2, 0)) && reached.contains(&(2, 2, 0)), "through the air beyond it: {reached:?}");
    assert!(!reached.contains(&(2, 0, 0)), "not through the stone: {reached:?}");
    // Camera in the air of the column waiting for its mesh.
    let from_inside = graph.run(column, [24.5, 40.0, 8.5], 0, 4, 4, true).to_vec();
    assert!(from_inside.contains(&(0, 2, 0)) && from_inside.contains(&(2, 2, 0)), "{from_inside:?}");
}
