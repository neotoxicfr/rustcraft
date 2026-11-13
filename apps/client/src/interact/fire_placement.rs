//! Where flint and steel may light a fire, as the game's client decides before placing it (and
//! playing the item's sound for its own player): an air cell where the fire holds, or one that
//! only an empty Nether portal frame around it allows, in the dimensions that have portals.

use rc_data::{VersionData, flags};

use super::FACES;
use crate::item_use::BlockTags;

/// The client's world as the fire rule reads it.
pub(super) struct FireWorld<'a, F: Fn([i32; 3]) -> u32> {
    pub data: &'a VersionData,
    pub tags: &'a BlockTags,
    pub state_at: F,
    pub min_y: i32,
}

impl<F: Fn([i32; 3]) -> u32> FireWorld<'_, F> {
    /// Flint and steel lights a fire at `p` for a player looking at `yaw`: the cell is air, and
    /// the fire holds there or an empty portal frame around it allows it (`portal_dimension`:
    /// the overworld or the Nether).
    pub(super) fn can_light_fire_at(&self, p: [i32; 3], yaw: f32, portal_dimension: bool) -> bool {
        if !self.is_air((self.state_at)(p)) {
            return false;
        }
        if self.fire_holds_at(p) {
            return true;
        }
        let cell = |q: [i32; 3]| self.portal_cell(q);
        portal_dimension && rc_sim::portal_frame_allows_fire(&cell, self.min_y, p, rc_sim::portal_axis_across_facing(yaw))
    }

    /// A fire at air cell `p` holds there: soul fire stands on its base blocks, fire on a sturdy
    /// top or beside a block that burns (not waterlogged).
    fn fire_holds_at(&self, p: [i32; 3]) -> bool {
        let below = (self.state_at)([p[0], p[1] - 1, p[2]]);
        if self.tagged("minecraft:soul_fire_base_blocks", below) || self.data.state(below).is_some_and(|i| i.sturdy & 2 != 0) {
            return true;
        }
        FACES.iter().any(|[dx, dy, dz]| {
            let s = (self.state_at)([p[0] + dx, p[1] + dy, p[2] + dz]);
            let burns = self.data.block_of(s).is_some_and(|b| rc_sim::fire_ignite_odds(&b.name) > 0);
            burns && self.data.state_properties(s).find(|(k, _)| *k == "waterlogged").map(|(_, v)| v) != Some("true")
        })
    }

    /// What the portal frame search sees at `q`: air or fire (inside), a portal block, a frame
    /// block, or anything else.
    fn portal_cell(&self, q: [i32; 3]) -> rc_sim::PortalCell {
        let s = (self.state_at)(q);
        if self.data.block_of(s).is_some_and(|b| b.name == "minecraft:nether_portal") {
            rc_sim::PortalCell::Portal
        } else if self.is_air(s) || self.tagged("minecraft:fire", s) {
            rc_sim::PortalCell::Empty
        } else if self.tagged("minecraft:nether_portal_frame", s) {
            rc_sim::PortalCell::Frame
        } else {
            rc_sim::PortalCell::Other
        }
    }

    fn is_air(&self, s: u32) -> bool {
        self.data.state(s).is_none_or(|i| i.has(flags::AIR))
    }

    /// The block of `state` is in synchronised tag `tag` (named with or without its namespace).
    fn tagged(&self, tag: &str, state: u32) -> bool {
        let Some(info) = self.data.state(state) else { return false };
        let b = u32::from(info.block);
        self.tags.contains(tag, b) || self.tags.contains(tag.trim_start_matches("minecraft:"), b)
    }
}

#[cfg(test)]
mod tests {
    use super::FireWorld;
    use crate::block_anim::tests::state_of;
    use crate::item_use::BlockTags;

    /// Obsidian frame along X with a 2 × 3 inside (x 1..2, y 1..3, z 0), nothing else around.
    fn frame_world(obsidian: u32, air: u32) -> impl Fn([i32; 3]) -> u32 {
        move |[x, y, z]: [i32; 3]| {
            let edge = x == 0 || x == 3 || y == 0 || y == 4;
            if z == 0 && (0..=3).contains(&x) && (0..=4).contains(&y) && edge { obsidian } else { air }
        }
    }

    /// Flint and steel inside an empty obsidian frame, clicked on its side at mid-height: no
    /// sturdy top below and nothing that burns beside, yet the game's client lights it there
    /// because the frame allows it (and plays the sound); not in the End, nor on a non-air cell
    /// or in plain air. Before, this client left that fire to the server and stayed silent.
    #[test]
    fn a_fire_only_a_portal_frame_allows_is_lit() {
        let Some(d) = rc_testdata::version_data() else { return };
        let block = |name: &str| i32::from(d.state(state_of(&d, name, &[])).expect("state").block);
        let mut tags = BlockTags::default();
        tags.set(&[
            ("minecraft:nether_portal_frame".into(), vec![block("minecraft:obsidian")]),
            ("minecraft:fire".into(), vec![block("minecraft:fire"), block("minecraft:soul_fire")]),
            ("minecraft:soul_fire_base_blocks".into(), vec![block("minecraft:soul_sand"), block("minecraft:soul_soil")]),
        ]);
        let (obsidian, air) = (state_of(&d, "minecraft:obsidian", &[]), state_of(&d, "minecraft:air", &[]));
        let world = FireWorld { data: &d, tags: &tags, state_at: frame_world(obsidian, air), min_y: -64 };
        let inside = [1, 2, 0];
        for yaw in [0.0, 90.0, 180.0, -90.0] {
            assert!(world.can_light_fire_at(inside, yaw, true), "frame-only fire, yaw {yaw}");
        }
        assert!(!world.can_light_fire_at(inside, 0.0, false), "no portal in the End");
        assert!(!world.can_light_fire_at([0, 2, 0], 0.0, true), "obsidian is not air");
        assert!(!world.can_light_fire_at([1, 2, 5], 0.0, true), "plain air holds no fire");
        // Above the frame's sturdy top a fire holds by itself, frame or not.
        assert!(world.can_light_fire_at([1, 5, 0], 0.0, false), "on obsidian");
    }
}
