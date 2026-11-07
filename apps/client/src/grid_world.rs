//! The loaded world as the physics sees it, and the small helpers that pick aim shapes and column grids.

use rc_world::ChunkGrid;

use crate::{block_entities, crack};

/// Aiming shape depending on the item held in the main hand (`held`): full light block
/// only with a light in hand (empty otherwise: neither aimed nor broken), full scaffolding with
/// scaffolding in hand; otherwise the extracted outline shape (`outline`).
fn held_shape(block: &str, held: &str, outline: u16, full: Option<u16>) -> u16 {
    match full {
        Some(f) if held == block && matches!(block, "minecraft:light" | "minecraft:scaffolding") => f,
        _ => outline,
    }
}

/// Aiming and outline shape of a state at `pos` for the local player: shape index depending on
/// the held item (`held`) and offset of the blocks whose shape follows the model (`crack::shape_shift`).
pub(crate) fn aim_shape(
    data: &rc_data::VersionData,
    full: Option<u16>,
    held: &str,
    state: u32,
    pos: [i32; 3],
) -> Option<(u16, [f64; 3])> {
    let info = data.state(state)?;
    let block = &data.blocks.get(usize::from(info.block))?.name;
    Some((held_shape(block, held, info.outline, full), crack::shape_shift(data, state, pos)))
}

/// Local player drawn: detached view, or lying (the camera is then on them even in first
/// person); never in free camera nor when the camera follows another entity.
pub(crate) fn draws_local_player(detached: bool, sleeping: bool, free_cam: bool, other_camera: bool) -> bool {
    (detached || sleeping) && !free_cam && !other_camera
}

/// Column grid of a joined world (connection, respawn, dimension change):
/// server view radius, at least 2.
pub(crate) fn level_grid(server_view: i32) -> ChunkGrid {
    ChunkGrid::new(server_view.max(2) as u32)
}

/// The loaded world as seen by the physics.
pub(crate) struct GridWorld<'a> {
    pub(crate) grid: &'a ChunkGrid,
    pub(crate) min_y: i32,
    pub(crate) height: i32,
    pub(crate) t: &'a rc_physics::Tables,
    pub(crate) border: &'a rc_physics::WorldBorder,
    /// Solid entities for the one who moves (`entity_motion::colliders`).
    pub(crate) colliders: &'a [rc_physics::Aabb],
    /// Moving piston blocks (shape depending on progress), and direction of the push in
    /// progress in which they do not block.
    pub(crate) movers: Option<&'a block_entities::BlockEntities>,
    pub(crate) noclip: Option<u8>,
}

impl GridWorld<'_> {
    /// Shape of a moving piston block at (x, y, z); false if it is not one.
    fn piston_shape(&self, s: u32, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        if !self.t.is_moving_piston(s) {
            return false;
        }
        if let Some(m) = self.movers.and_then(|b| b.mover([x, y, z])) {
            self.t.moving_collision(&m, self.noclip, out);
        }
        true
    }

    /// State at (x, y, z) in its column, already found (`None`: not loaded). Unlike the server,
    /// an unloaded column stays `None` even outside the world's height.
    fn state_in(&self, column: Option<&rc_world::Column>, x: i32, y: i32, z: i32) -> Option<u32> {
        let c = column?;
        let ly = y - self.min_y;
        if !(0..self.height).contains(&ly) {
            return Some(0);
        }
        c.block((x & 15) as usize, ly as usize, (z & 15) as usize)
    }

    /// Could one of the loaded sections holding cells of `lo..=hi` hold a state satisfying `pred`?
    /// Answered by the section palettes, without reading a block; unloaded columns hold nothing
    /// (their cells have no collision).
    fn sections_may_hold(&self, lo: [i32; 3], hi: [i32; 3], pred: &dyn Fn(u32) -> bool) -> bool {
        let bottom = (lo[1] - self.min_y).max(0);
        let top = (hi[1] - self.min_y).min(self.height - 1);
        if bottom > top {
            return false;
        }
        let sections = (bottom >> 4) as usize..=(top >> 4) as usize;
        (lo[0] >> 4..=hi[0] >> 4).any(|cx| {
            (lo[2] >> 4..=hi[2] >> 4).any(|cz| {
                let Some(c) = self.grid.get(cx, cz) else { return false };
                sections.clone().any(|i| c.sections.get(i).is_some_and(|s| s.blocks.may_contain(pred)))
            })
        })
    }
}

impl rc_physics::World for GridWorld<'_> {
    fn entity_colliders(&self, area: &rc_physics::Aabb, out: &mut Vec<rc_physics::Aabb>) {
        rc_physics::entity_boxes_in(self.colliders, area, out);
    }

    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        match (self.movers, self.state(x, y, z)) {
            (Some(b), Some(s)) if b.moving > 0 => self.piston_shape(s, x, y, z, out),
            _ => false,
        }
    }

    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.state_in(self.grid.get(x >> 4, z >> 4), x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        self.collision_of_state(self.state(x, y, z), x, y, z, out)
    }

    /// One grid lookup per column instead of one per cell.
    fn states_in(&self, first: [i32; 3], last: [i32; 3], out: &mut [u32]) -> bool {
        let (nx, nz) = ((last[0] - first[0] + 1) as usize, (last[2] - first[2] + 1) as usize);
        for cx in first[0] >> 4..=last[0] >> 4 {
            for cz in first[2] >> 4..=last[2] >> 4 {
                let column = self.grid.get(cx, cz);
                let xs = first[0].max(cx << 4)..=last[0].min((cx << 4) + 15);
                let zs = first[2].max(cz << 4)..=last[2].min((cz << 4) + 15);
                for y in first[1]..=last[1] {
                    for z in zs.clone() {
                        let row = ((y - first[1]) as usize * nz + (z - first[2]) as usize) * nx;
                        for x in xs.clone() {
                            out[row + (x - first[0]) as usize] = self.state_in(column, x, y, z).unwrap_or(rc_physics::UNLOADED);
                        }
                    }
                }
            }
        }
        true
    }

    fn collision_of_state(&self, state: Option<u32>, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        match state {
            Some(s) => (self.movers.is_some_and(|b| b.moving > 0) && self.piston_shape(s, x, y, z, out)) || self.t.collision(s, out),
            None => false,
        }
    }

    fn border(&self) -> Option<&rc_physics::WorldBorder> {
        Some(self.border)
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn shape_reach(&self, lo: [i32; 3], hi: [i32; 3]) -> rc_physics::Aabb {
        // A moving block's shape follows its progress out of its cell.
        if self.movers.is_some_and(|b| b.moving > 0) {
            return rc_physics::Aabb::EVERYWHERE;
        }
        if rc_data::opt_switch!("RUSTCRAFT_OPT_COLLISION_PALETTE_PREFILTER") {
            return self.t.reach_from_palettes(|pred| self.sections_may_hold(lo, hi, pred));
        }
        self.t.shape_reach()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_world::Column;

    fn col(x: i32, z: i32) -> Column {
        Column {
            x,
            z,
            sections: Box::new([]),
            sky_light: Box::new([]),
            block_light: Box::new([]),
            heightmaps: Vec::new(),
        }
    }

    /// Column of 4 sections: a single state, a packed mix, air, a single state again.
    fn filled(x: i32, z: i32) -> Column {
        use rc_world::{Container, Section};
        let mixed: Vec<u32> = (0..4096u32).map(|i| (i * 7 + x as u32 * 3 + z as u32) % 11).collect();
        let section = |blocks| Section { non_empty: 0, blocks, biomes: Container::Single(0) };
        let sections = vec![
            section(Container::Single(1)),
            section(Container::pack(&mixed, 4, 8, 15)),
            section(Container::Single(0)),
            section(Container::Single(5)),
        ];
        Column { sections: sections.into_boxed_slice(), ..col(x, z) }
    }

    /// `RUSTCRAFT_OPT_COLLISION_BATCH_READ`: the states of a span, read once per column, are the
    /// ones `state` gives cell by cell, across columns, unloaded ones and the world's height.
    #[test]
    fn states_in_matches_state() {
        let Some(d) = rc_testdata::version_data() else { return };
        let t = rc_physics::Tables::new(&d);
        let mut grid = ChunkGrid::new(4);
        for (x, z) in [(0, 0), (1, 0), (0, 1)] {
            grid.insert(filled(x, z));
        }
        let border = rc_physics::WorldBorder::default();
        let w = GridWorld { grid: &grid, min_y: -64, height: 64, t: &t, border: &border, colliders: &[], movers: None, noclip: None };
        use rc_physics::World;
        let mut cells = 0;
        for (first, last) in [([10, -70, 10], [20, -60, 20]), ([-3, -2, 14], [17, 3, 17]), ([14, -20, -2], [33, -15, 2]), ([5, -40, 5], [5, -40, 5])] {
            let n = (0..3).map(|a| (last[a] - first[a] + 1) as usize).product();
            let mut out = vec![0; n];
            assert!(w.states_in(first, last, &mut out));
            let mut i = 0;
            for y in first[1]..=last[1] {
                for z in first[2]..=last[2] {
                    for x in first[0]..=last[0] {
                        assert_eq!(out[i], w.state(x, y, z).unwrap_or(rc_physics::UNLOADED), "({x}, {y}, {z})");
                        i += 1;
                    }
                }
            }
            cells += n;
        }
        assert!(cells > 2000);
    }

    /// Respawn with a server view of 32: the grid keeps columns 0 and 35 side by side
    /// (a grid of radius 16, 35 cells wide, put them in the same cell). Covers the function
    /// alone: its call in the respawn is not tested (no harness for `App`).
    #[test]
    fn level_grid_follows_server_view() {
        let mut g = level_grid(32);
        g.insert(col(0, 0));
        g.insert(col(35, 0));
        assert!(g.get(0, 0).is_some() && g.get(35, 0).is_some());
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn sleeping_local_player_is_drawn_in_first_person() {
        assert!(draws_local_player(false, true, false, false));
        assert!(!draws_local_player(false, false, false, false));
        assert!(draws_local_player(true, false, false, false));
        assert!(!draws_local_player(true, true, true, false) && !draws_local_player(true, true, false, true));
    }

    /// Light block aimable only with a light in hand; full scaffolding with scaffolding in hand.
    #[test]
    fn held_item_changes_aim_shape() {
        let full = Some(1);
        assert_eq!(held_shape("minecraft:light", "minecraft:light", 0, full), 1);
        assert_eq!(held_shape("minecraft:light", "minecraft:stone", 0, full), 0);
        assert_eq!(held_shape("minecraft:scaffolding", "minecraft:scaffolding", 707, full), 1);
        assert_eq!(held_shape("minecraft:scaffolding", "", 707, full), 707);
        assert_eq!(held_shape("minecraft:stone", "minecraft:stone", 1, full), 1);
    }
}
