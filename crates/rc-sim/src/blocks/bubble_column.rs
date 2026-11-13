//! Bubble columns: a column of full source water above soul sand (pushes up) or a magma block
//! (drags down) turns into bubble column cells, follows its support and goes back to water
//! without it; then its push on the entities that cross it.
//!
//! Since 26.3 the water block itself starts a column: a full source water placed, or warned
//! by a neighbour, or whose block below changed, plans its own block tick in 20 when the block
//! below enables a column, and that tick builds the column upwards. A column cell checks
//! itself in 5 ticks when it may no longer hold (support gone, the cell below changed) or when
//! a source water it could take appears above. Building writes the cell then climbs while the
//! next cell can be taken, stopping at the first one already in that state.

use rc_data::PropKey;
use rc_physics::Aabb;

use super::{Dir, Kind, P, above, below, flag};
use crate::inside_blocks::Who;
use crate::server::Server;
use crate::tag_bits::TagId;

/// Delay of the water block's tick that builds a column.
const WATER_BUILDS_COLUMN_DELAY: i32 = 20;
/// Delay of a column cell's own check.
const COLUMN_CHECK_DELAY: i32 = 5;
/// Fluid type of source water in the fluid tick queue.
const WATER_FLUID: u8 = 1;

/// Vertical speed limits and steps of the push: above the column top (nothing over it) or inside.
const ABOVE_UP_STEP: f64 = 0.1;
const ABOVE_UP_LIMIT: f64 = 1.8;
const ABOVE_DOWN_STEP: f64 = 0.03;
const ABOVE_DOWN_LIMIT: f64 = -0.9;
const INSIDE_UP_STEP: f64 = 0.06;
const INSIDE_UP_LIMIT: f64 = 0.7;
const INSIDE_DOWN_STEP: f64 = 0.03;
const INSIDE_DOWN_LIMIT: f64 = -0.3;

/// New vertical speed of an entity in a column cell (`above`: nothing over the cell).
pub(crate) fn pushed_speed(vy: f64, drag_down: bool, above: bool) -> f64 {
    match (above, drag_down) {
        (true, true) => (vy - ABOVE_DOWN_STEP).max(ABOVE_DOWN_LIMIT),
        (true, false) => (vy + ABOVE_UP_STEP).min(ABOVE_UP_LIMIT),
        (false, true) => (vy - INSIDE_DOWN_STEP).max(INSIDE_DOWN_LIMIT),
        (false, false) => (vy + INSIDE_UP_STEP).min(INSIDE_UP_LIMIT),
    }
}

impl Server {
    // --- Forming and undoing. ---

    /// The direction a column takes from the block below: `Some(true)` drags down (magma),
    /// `Some(false)` pushes up (soul sand), `None` carries no column.
    fn column_support(&self, below: u32) -> Option<bool> {
        let b = self.view().block(below);
        if self.tag_id(TagId::EnablesBubbleColumnPushUp, b) {
            Some(false)
        } else if self.tag_id(TagId::EnablesBubbleColumnDragDown, b) {
            Some(true)
        } else {
            None
        }
    }

    /// A full source water block (the only liquid a column may take).
    fn is_column_water(&self, s: u32) -> bool {
        self.view().kind(s) == Kind::Water && self.fluid_of(s).source
    }

    /// A cell a column can take: a column already, or a full source water block.
    fn column_can_occupy(&self, s: u32) -> bool {
        self.view().block(s) == self.bs.t.id.bubble_column || self.is_column_water(s)
    }

    /// State a column cell takes over `below`: the column below continued, a new column on its
    /// support, otherwise water again (a column cell) or unchanged (a water cell).
    fn column_state(&self, below: u32, occupied: u32) -> u32 {
        let v = self.view();
        let column = self.bs.t.id.bubble_column;
        if v.block(below) == column {
            return below;
        }
        match self.column_support(below) {
            Some(drag_down) => v.set_bool(v.default_state(column), PropKey::Drag, drag_down),
            None if v.block(occupied) == column => v.default_state(self.bs.t.id.water),
            None => occupied,
        }
    }

    /// Builds (or undoes) the column from `at` upwards, given the cell's state and the one below.
    fn update_column(&mut self, at: P, occupied: u32, below: u32) {
        if !self.column_can_occupy(occupied) {
            return;
        }
        let state = self.column_state(below, occupied);
        self.set_block(at, state, flag::CLIENTS);
        let mut q = above(at);
        while self.column_can_occupy(self.get(q)) {
            // The first cell already in that state ends the climb.
            if !self.set_block(q, state, flag::CLIENTS) {
                return;
            }
            q = above(q);
        }
    }

    /// Water block placed or warned by a neighbour: a full source over a column support plans
    /// its column (block tick of the water in 20). The block below is read only for a full
    /// source (flowing water, the common case of these updates, costs one table read).
    pub(crate) fn water_plan_column(&mut self, s: u32, p: P) {
        if self.is_column_water(s) {
            let under = self.get(below(p));
            self.plan_column_over(s, p, under);
        }
    }

    /// Water block whose block below just became `below` (shape update from below).
    pub(crate) fn water_plan_column_over(&mut self, s: u32, p: P, below: u32) {
        if self.is_column_water(s) {
            self.plan_column_over(s, p, below);
        }
    }

    /// The water's block tick in 20 when `below` carries a column.
    fn plan_column_over(&mut self, water: u32, p: P, below: u32) {
        if self.column_support(below).is_some() {
            let block = self.view().block(water);
            self.schedule_tick(p, block, WATER_BUILDS_COLUMN_DELAY);
        }
    }

    /// Block tick of a water block (not its fluid tick): the column it planned.
    pub(crate) fn water_block_tick(&mut self, s: u32, p: P) {
        if self.is_column_water(s) {
            let under = self.get(below(p));
            self.update_column(p, s, under);
        }
    }

    /// Scheduled check of a column cell: rebuilt from what is below it now.
    pub(crate) fn bubble_column_tick(&mut self, s: u32, p: P) {
        let under = self.get(below(p));
        self.update_column(p, s, under);
    }

    /// Shape update of a column cell (neighbour towards `dir` now `ns`): the water's own tick,
    /// and the cell's check in 5 when it may have to change.
    pub(crate) fn bubble_column_update_shape(&mut self, s: u32, p: P, dir: Dir, ns: u32) -> u32 {
        let delay = self.fluid_delay(WATER_FLUID);
        self.schedule_fluid_tick(p, WATER_FLUID, delay);
        let column = self.bs.t.id.bubble_column;
        let survives = {
            let under = self.get(below(p));
            self.view().block(under) == column || self.column_support(under).is_some()
        };
        let water_appeared_above = dir == Dir::Up && self.view().block(ns) != column && self.column_can_occupy(ns);
        if !survives || dir == Dir::Down || water_appeared_above {
            self.schedule_tick(p, column, COLUMN_CHECK_DELAY);
        }
        s
    }

    // --- Push on the entities. ---

    /// Nothing over the column cell `p`: no collision and no fluid above (the push is then the
    /// stronger "above" one, with splashes).
    fn column_open_above(&self, p: P) -> bool {
        let up = self.get(above(p));
        self.phys.shape(up).is_empty() && self.phys.fluids.fluid(up) == rc_physics::Fluid::None
    }

    /// Splashes and bubbles over a column's top cell (two of each, world random), sent by the
    /// server whenever an entity is pushed there.
    fn column_splashes(&mut self, p: P) {
        let y = f64::from(p[1] + 1);
        for _ in 0..2 {
            let (sx, sz) = (self.mobs.rng.next_double(), self.mobs.rng.next_double());
            self.particles("minecraft:splash", [f64::from(p[0]) + sx, y, f64::from(p[2]) + sz], [0.0; 3], 1.0, 1);
            let (bx, bz) = (self.mobs.rng.next_double(), self.mobs.rng.next_double());
            self.particles("minecraft:bubble", [f64::from(p[0]) + bx, y, f64::from(p[2]) + bz], [0.0, 0.01, 0.0], 0.2, 1);
        }
    }

    /// Column cell `p` precisely entered by `who` during the crossed-block pass. The server moves
    /// what it simulates (mobs not steered by the player; an item's or orb's own physics
    /// already pushed it), a flying player is left alone, and the splashes and the forgotten
    /// fall follow the game's rule for each case.
    pub(crate) fn bubble_column_inside(&mut self, who: &mut Who, p: P, drag_down: bool) {
        let open = self.column_open_above(p);
        match who {
            Who::Player => {
                if self.abilities().flying {
                    return;
                }
                if !open {
                    self.life.vit.fall = 0.0;
                }
            }
            Who::Mob(m) => {
                if !Self::player_controls(m) {
                    m.vel[1] = pushed_speed(m.vel[1], drag_down, open);
                }
                if !open {
                    m.fall_distance = 0.0;
                }
            }
            // Their physics (`rc_physics::body`) already applied the speed and the fall.
            Who::Item(_) | Who::Orb(_) => {}
        }
        if open {
            self.column_splashes(p);
        }
    }

    /// Column cells a minecart's box touches: pushed like any entity the server moves, with the
    /// splashes over the top cell.
    pub(crate) fn cart_bubble_column(&mut self, c: &mut crate::minecart::Cart, p: P, drag_down: bool) {
        let open = self.column_open_above(p);
        c.vel[1] = pushed_speed(c.vel[1], drag_down, open);
        if open {
            self.column_splashes(p);
        } else {
            c.fall_distance = 0.0;
        }
    }
}

/// The swept box precisely enters a column cell (its push applies): the segment is long, or
/// the box at its end (shrunk by 1e-5) overlaps the cell.
pub(crate) fn column_entered_precisely(target: &Aabb, travel: [f64; 3], cell: P) -> bool {
    let far = travel[0] * travel[0] + travel[1] * travel[1] + travel[2] * travel[2] > 0.999_990_000_000_252_6f64.powi(2);
    let o = cell.map(f64::from);
    far || target.intersects(&Aabb { min: o, max: [o[0] + 1.0, o[1] + 1.0, o[2] + 1.0] })
}
