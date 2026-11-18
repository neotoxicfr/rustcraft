//! Entities that block building, as the game flags them: every living entity (a spectator and a
//! marker armor stand aside), a boat, a minecart, primed TNT, a falling block and an End crystal.
//! One of them in the way keeps a block from being placed there (by a player, the placing one
//! included, or by a dispenser) and a mob from spawning there.

use rc_physics::Aabb;

use crate::server::Server;

/// No entity to leave out of the search (a placement is made for no entity).
pub(crate) const NO_ENTITY: i32 = -1;

impl Server {
    /// An entity that blocks building, other than `except`, whose box overlaps `area` and passes
    /// `overlaps` too (a placement tests there each box of the placed block's shape; `area` is
    /// their bounds). The cheap lists are walked first.
    pub(crate) fn building_blocker_in(&self, area: &Aabb, except: i32, overlaps: impl Fn(&Aabb) -> bool) -> bool {
        let hit = |b: &Aabb| b.intersects(area) && overlaps(b);
        self.player_blocks_building(&hit)
            || self.mob_in_the_way(area, except, crate::mobs::spawn::obstruction_bound_first(), &overlaps)
            || self.moving_block_blocks_building(&hit)
            || self.carts.list.iter().flatten().any(|c| !c.removed && hit(&c.bbox()))
            || self.mobs.end.crystals.iter().any(|c| !c.removed && hit(&c.bbox()))
            || self.stand_blocks_building(&hit)
    }

    /// The current player or another one standing in this world, not a spectator, even dead.
    fn player_blocks_building(&self, hit: &impl Fn(&Aabb) -> bool) -> bool {
        if self.game_mode != 3 && self.player_stands_here() && hit(&self.player_aabb()) {
            return true;
        }
        self.lan_others_here().any(|c| c.game_mode != 3 && hit(&crate::mobs::guest_box(c)))
    }

    /// A falling block or primed TNT (both 0.98 wide and high).
    fn moving_block_blocks_building(&self, hit: &impl Fn(&Aabb) -> bool) -> bool {
        const HALF: f64 = 0.98 / 2.0;
        self.bs.entities.list.iter().any(|e| {
            let p = e.pos;
            !e.removed && hit(&Aabb { min: [p[0] - HALF, p[1], p[2] - HALF], max: [p[0] + HALF, p[1] + 0.98, p[2] + HALF] })
        })
    }

    /// An entity that blocks building stands in the collision shape of `state` placed at `p`.
    pub(crate) fn placement_obstructed(&self, state: u32, p: [i32; 3]) -> bool {
        let shape = self.data.shape(self.view().info(state).collision);
        let corner = p.map(f64::from);
        let at = |b: &rc_data::Aabb| Aabb {
            min: [b.min[0] + corner[0], b.min[1] + corner[1], b.min[2] + corner[2]],
            max: [b.max[0] + corner[0], b.max[1] + corner[1], b.max[2] + corner[2]],
        };
        let Some(first) = shape.first() else { return false };
        // The search runs over the shape's bounds; each entity found is then held against every box.
        let bounds = shape.iter().map(at).fold(at(first), |u, b| Aabb {
            min: [0, 1, 2].map(|i| u.min[i].min(b.min[i])),
            max: [0, 1, 2].map(|i| u.max[i].max(b.max[i])),
        });
        self.building_blocker_in(&bounds, NO_ENTITY, |e| shape.iter().any(|b| at(b).intersects(e)))
    }
}
