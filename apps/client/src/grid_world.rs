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
    /// `RUSTCRAFT_OPT_COLLISION_PALETTE_PREFILTER` ([`palette_prefilter`]), kept per world so the
    /// tests compare both paths.
    pub(crate) palette_prefilter: bool,
}

/// `RUSTCRAFT_OPT_COLLISION_PALETTE_PREFILTER` (default on): the collision walk asks the section
/// palettes which shapes can be there before choosing the cells it reads.
pub(crate) fn palette_prefilter() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_COLLISION_PALETTE_PREFILTER")
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
        let state_in = |column, [x, y, z]: [i32; 3]| self.state_in(column, x, y, z).unwrap_or(rc_physics::UNLOADED);
        rc_world::states_by_column(first, last, out, |cx, cz| self.grid.get(cx, cz), state_in);
        true
    }

    fn collision_of_state(&self, state: Option<u32>, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        match state {
            Some(s) => {
                (self.movers.is_some_and(|b| b.moving > 0) && self.piston_shape(s, x, y, z, out))
                    || self.t.collision(s, out)
            }
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
        if self.palette_prefilter {
            let sections = |cx, cz| self.grid.get(cx, cz).map(|c| &*c.sections);
            return self.t.reach_from_palettes(|pred| {
                rc_world::sections_may_hold(lo, hi, self.min_y, self.height, sections, pred)
            });
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
        let w = GridWorld {
            grid: &grid,
            min_y: -64,
            height: 64,
            t: &t,
            border: &border,
            colliders: &[],
            movers: None,
            noclip: None,
            palette_prefilter: true,
        };
        use rc_physics::World;
        let mut cells = 0;
        for (first, last) in [
            ([10, -70, 10], [20, -60, 20]),
            ([-3, -2, 14], [17, 3, 17]),
            ([14, -20, -2], [33, -15, 2]),
            ([5, -40, 5], [5, -40, 5]),
        ] {
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

    /// Every state of the named blocks.
    fn states_of(d: &rc_data::VersionData, names: &[&str]) -> Vec<u32> {
        names
            .iter()
            .flat_map(|name| {
                let b = d.blocks.iter().find(|b| b.name == *name).expect(name);
                b.first_state..b.first_state + b.state_count
            })
            .collect()
    }

    /// A section whose cells are drawn among `states` (packed with a palette), fixed randomness.
    fn scattered(states: &[u32], seed: u64) -> rc_world::Container {
        let mut r = seed;
        let values: Vec<u32> = (0..4096)
            .map(|_| {
                r = r.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                states[(r >> 33) as usize % states.len()]
            })
            .collect();
        rc_world::Container::pack(&values, 4, 8, 15)
    }

    fn column_of(x: i32, z: i32, blocks: [rc_world::Container; 4]) -> Column {
        let section = |blocks| rc_world::Section { non_empty: 0, blocks, biomes: rc_world::Container::Single(0) };
        Column { sections: blocks.map(section).into(), ..col(x, z) }
    }

    /// Boxes of every size around and between the test columns, in the sky and below the world.
    fn test_boxes() -> Vec<rc_physics::Aabb> {
        let mut r = 0x0061_21D5u64;
        let mut f = || {
            r = r.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (r >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..8_000)
            .map(|_| {
                let min = [-8.0 + f() * 48.0, -72.0 + f() * 80.0, -8.0 + f() * 48.0];
                let size = [0.1 + f() * 1.4, 0.1 + f() * 1.9, 0.1 + f() * 1.4];
                rc_physics::Aabb { min, max: std::array::from_fn(|a| min[a] + size[a]) }
            })
            .collect()
    }

    /// `RUSTCRAFT_OPT_COLLISION_SCAN_TRIM` and `RUSTCRAFT_OPT_COLLISION_PALETTE_PREFILTER` on the
    /// client's world: the trimmed walk keeps every box of the whole one, prefilter off then on.
    /// Columns: (0, 0) mixes fences, piston heads and pitcher crops (shapes spilling out of their
    /// cell) and keeps a piston head in the palette of an empty section; (1, 0) holds only shapes
    /// that stay in their cell sideways; (0, 1) is air over stone; (1, 1) is not loaded.
    #[test]
    fn grid_world_walk_keeps_every_box() {
        use rc_physics::{Aabb, World, collide};
        use rc_world::Container;
        let Some(d) = rc_testdata::version_data() else { return };
        let t = rc_physics::Tables::new(&d);
        let [stone] = states_of(&d, &["minecraft:stone"])[..] else { panic!("one stone state") };
        let air_and_stone = [0, stone];
        let spilling = [
            &air_and_stone[..],
            &states_of(
                &d,
                &["minecraft:oak_fence", "minecraft:piston_head", "minecraft:pitcher_crop", "minecraft:oak_slab"],
            ),
        ]
        .concat();
        let in_cell = [
            &air_and_stone[..],
            &states_of(&d, &["minecraft:oak_fence", "minecraft:oak_slab", "minecraft:short_grass"]),
        ]
        .concat();
        // A piston head placed then removed: the section stays empty, its palette keeps it.
        let head = states_of(&d, &["minecraft:piston_head"])[0];
        let mut stale = Container::Single(0);
        stale.set(rc_world::block_index(3, 3, 3), head, 4, 8, 15);
        stale.set(rc_world::block_index(3, 3, 3), 0, 4, 8, 15);
        assert!(stale.may_contain(|s| s == head) && (0..4096).all(|i| stale.get(i) == 0), "stale palette entry");
        let mut grid = ChunkGrid::new(4);
        grid.insert(column_of(0, 0, [Container::Single(stone), scattered(&spilling, 1), Container::Single(0), stale]));
        grid.insert(column_of(
            1,
            0,
            [Container::Single(stone), scattered(&in_cell, 2), Container::Single(0), Container::Single(stone)],
        ));
        grid.insert(column_of(
            0,
            1,
            [Container::Single(stone), Container::Single(0), Container::Single(0), Container::Single(0)],
        ));
        let border = rc_physics::WorldBorder::default();
        let (mut found, mut nothing, mut common, mut whole_reach) = (0, 0, 0, 0);
        let (mut trimmed, mut whole) = (Vec::new(), Vec::new());
        for palette_prefilter in [false, true] {
            let w = GridWorld {
                grid: &grid,
                min_y: -64,
                height: 64,
                t: &t,
                border: &border,
                colliders: &[],
                movers: None,
                noclip: None,
                palette_prefilter,
            };
            for b in test_boxes() {
                collide::collect(&w, &b, &mut trimmed);
                collide::collect_untrimmed(&w, &b, &mut whole);
                assert_eq!(trimmed, whole, "box {b:?}, prefilter {palette_prefilter}");
                found += whole.len();
                let lo = b.min.map(|v| (v - collide::EPS).floor() as i32 - 1);
                let hi = b.max.map(|v| (v + collide::EPS).floor() as i32 + 1);
                let reach = w.shape_reach(lo, hi);
                nothing += usize::from(palette_prefilter && reach == Aabb::NOTHING);
                common += usize::from(palette_prefilter && reach == t.common_shape_reach());
                whole_reach += usize::from(palette_prefilter && reach == t.shape_reach());
            }
        }
        assert!(
            found > 10_000 && nothing > 300 && common > 100 && whole_reach > 200,
            "coverage: {found} boxes, reach: {nothing} nothing, {common} common, {whole_reach} whole"
        );
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
