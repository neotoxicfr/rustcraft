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
        let c = self.grid.get(x >> 4, z >> 4)?;
        let ly = y - self.min_y;
        if !(0..self.height).contains(&ly) {
            return Some(0);
        }
        c.block((x & 15) as usize, ly as usize, (z & 15) as usize)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        match self.state(x, y, z) {
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
