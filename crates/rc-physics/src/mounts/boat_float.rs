//! Water status and flotation of a boat or raft (`docs/specs/mounts.md` § 3.2), shared by the boat
//! the local player pilots (client, [`super::Mount`]) and the boats the integrated server simulates.

use super::Status;
use crate::{Aabb, Fluid, Tables, World, is_free};

/// Gravity the buoyancy is scaled by: the boat's default gravity, even when it has none.
const DEFAULT_GRAVITY: f64 = 0.04;

/// What the flotation keeps from one tick to the next.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BoatFloat {
    /// Status of this tick, and of the previous one (none before the second reading: a new boat
    /// has no status, so it never splashes down on its first tick).
    pub status: Status,
    pub old_status: Option<Status>,
    /// The status has been read at least once.
    status_read: bool,
    /// Ticks spent under water in a row (the passengers are thrown out at 60).
    pub out_of_control: f32,
    /// Rotation per tick (degrees), set by the piloting and damped by the flotation.
    pub rotation: f32,
    /// Water surface found by the last status reading.
    water_level: f64,
    /// Average friction of the ground under the boat, halved each tick a player pilots it on land.
    land_friction: f32,
    /// Vertical speed at the last move: how far above the boat the surface is looked for.
    last_vy: f64,
}

/// Position, velocity and size of a boat, handed to the flotation.
#[derive(Debug, Clone, Copy)]
pub struct BoatBody {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub width: f64,
    pub height: f64,
}

impl BoatBody {
    fn bounding_box(&self) -> Aabb {
        let r = self.width / 2.0;
        Aabb {
            min: [self.pos[0] - r, self.pos[1], self.pos[2] - r],
            max: [self.pos[0] + r, self.pos[1] + self.height, self.pos[2] + r],
        }
    }
}

/// Water of block (x, y, z): its height in the block and whether it is a source.
fn water_at(t: &Tables, w: &dyn World, x: i32, y: i32, z: i32) -> Option<(f32, bool)> {
    let s = w.state(x, y, z).unwrap_or(0);
    if t.fluids.fluid(s) != Fluid::Water {
        return None;
    }
    let above = w.state(x, y + 1, z).unwrap_or(0);
    Some((t.fluids.height_at(s, above), t.fluids.source.get(s as usize).copied().unwrap_or(false)))
}

impl BoatFloat {
    /// Start of the boat's tick: the previous status kept, the new one read around box `b`, and
    /// the count of ticks spent under water.
    pub fn update_status(&mut self, b: &Aabb, w: &dyn World, t: &Tables) {
        self.old_status = self.status_read.then_some(self.status);
        self.status_read = true;
        self.status = self.read_status(b, w, t);
        if self.is_under_water() {
            self.out_of_control += 1.0;
        } else {
            self.out_of_control = 0.0;
        }
    }

    /// The status read now around box `b`, the boat's state unchanged (the paddle sound reads it
    /// again after the move; what that reading changes in the game is read again before any use).
    pub fn status_now(&self, b: &Aabb, w: &dyn World, t: &Tables) -> Status {
        let mut copy = *self;
        copy.read_status(b, w, t)
    }

    /// Under water, still or flowing.
    pub fn is_under_water(&self) -> bool {
        matches!(self.status, Status::UnderWater | Status::UnderFlowingWater)
    }

    /// The vertical speed at the end of a move (kept by the fall check of the move).
    pub fn remember_vertical_speed(&mut self, vy: f64) {
        self.last_vy = vy;
    }

    fn read_status(&mut self, b: &Aabb, w: &dyn World, t: &Tables) -> Status {
        if let Some(under) = self.under_water_status(b, w, t) {
            self.water_level = b.max[1];
            return under;
        }
        if self.check_in_water(b, w, t) {
            return Status::InWater;
        }
        let friction = ground_friction(b, w, t);
        // NaN (no block under the boat) fails the test, like the game's 0 / 0.
        if friction > 0.0 {
            self.land_friction = friction;
            Status::OnLand
        } else {
            Status::InAir
        }
    }

    /// Water above the top of the box: flowing water wins over a source.
    fn under_water_status(&self, b: &Aabb, w: &dyn World, t: &Tables) -> Option<Status> {
        let top = b.max[1] + 0.001;
        let mut under = false;
        for x in b.min[0].floor() as i32..b.max[0].ceil() as i32 {
            for y in b.max[1].floor() as i32..top.ceil() as i32 {
                for z in b.min[2].floor() as i32..b.max[2].ceil() as i32 {
                    if let Some((h, source)) = water_at(t, w, x, y, z)
                        && top < f64::from(y) + f64::from(h)
                    {
                        if !source {
                            return Some(Status::UnderFlowingWater);
                        }
                        under = true;
                    }
                }
            }
        }
        under.then_some(Status::UnderWater)
    }

    /// Water at the bottom of the box; the highest surface met is kept.
    fn check_in_water(&mut self, b: &Aabb, w: &dyn World, t: &Tables) -> bool {
        let mut in_water = false;
        self.water_level = -f64::MAX;
        for x in b.min[0].floor() as i32..b.max[0].ceil() as i32 {
            for y in b.min[1].floor() as i32..(b.min[1] + 0.001).ceil() as i32 {
                for z in b.min[2].floor() as i32..b.max[2].ceil() as i32 {
                    if let Some((h, _)) = water_at(t, w, x, y, z) {
                        let level = y as f32 + h;
                        self.water_level = self.water_level.max(f64::from(level));
                        in_water |= b.min[1] < f64::from(level);
                    }
                }
            }
        }
        in_water
    }

    /// Water surface above the boat, for a splashdown.
    fn water_level_above(&self, b: &Aabb, w: &dyn World, t: &Tables) -> f32 {
        let (x0, x1) = (b.min[0].floor() as i32, b.max[0].ceil() as i32);
        let (z0, z1) = (b.min[2].floor() as i32, b.max[2].ceil() as i32);
        let y0 = b.max[1].floor() as i32;
        let y1 = (b.max[1] - self.last_vy).ceil() as i32;
        'layer: for y in y0..y1 {
            let mut h = 0.0f32;
            for x in x0..x1 {
                for z in z0..z1 {
                    if let Some((wh, _)) = water_at(t, w, x, y, z) {
                        h = h.max(wh);
                    }
                    if h >= 1.0 {
                        continue 'layer;
                    }
                }
            }
            if h < 1.0 {
                return y as f32 + h;
            }
        }
        (y1 + 1) as f32
    }

    /// Buoyancy, gravity and friction of the status. `gravity`: the boat's own (0 without
    /// gravity); `piloted_by_player`: the land friction is halved each tick a player pilots it.
    pub fn float(&mut self, body: &mut BoatBody, gravity: f64, piloted_by_player: bool, w: &dyn World, t: &Tables) {
        let mut vspeed = -gravity;
        let mut buoyancy = 0.0f64;
        let k;
        if self.old_status == Some(Status::InAir) && !matches!(self.status, Status::InAir | Status::OnLand) {
            self.splash_down(body, w, t);
            return;
        }
        match self.status {
            Status::InWater => {
                buoyancy = (self.water_level - body.pos[1]) / body.height;
                k = 0.9;
            }
            Status::UnderFlowingWater => {
                vspeed = -7.0e-4;
                k = 0.9;
            }
            Status::UnderWater => {
                buoyancy = f64::from(0.01f32);
                k = 0.45;
            }
            Status::InAir => k = 0.9,
            Status::OnLand => {
                k = self.land_friction;
                if piloted_by_player {
                    self.land_friction /= 2.0;
                }
            }
        }
        let kf = f64::from(k);
        body.vel[0] *= kf;
        body.vel[1] += vspeed;
        body.vel[2] *= kf;
        self.rotation *= k;
        if buoyancy > 0.0 {
            body.vel[1] = (body.vel[1] + buoyancy * (DEFAULT_GRAVITY / 0.65)) * 0.75;
        }
    }

    /// Falling into water: set on the surface if there is room, without damping this tick.
    fn splash_down(&mut self, body: &mut BoatBody, w: &dyn World, t: &Tables) {
        self.water_level = body.pos[1] + body.height;
        let b = body.bounding_box();
        let target = f64::from(self.water_level_above(&b, w, t)) - body.height + 0.101;
        if is_free(w, &b.offset(0.0, target - body.pos[1], 0.0)) {
            body.pos[1] = target;
            body.vel[1] = 0.0;
            self.last_vy = 0.0;
        }
        self.status = Status::InWater;
    }
}

/// Average friction of the blocks under box `b` (lily pads left out); NaN without a block.
fn ground_friction(b: &Aabb, w: &dyn World, t: &Tables) -> f32 {
    let slab = Aabb { min: [b.min[0], b.min[1] - 0.001, b.min[2]], max: [b.max[0], b.min[1], b.max[2]] };
    let (x0, x1) = (slab.min[0].floor() as i32 - 1, slab.max[0].ceil() as i32 + 1);
    let (y0, y1) = (slab.min[1].floor() as i32 - 1, slab.max[1].ceil() as i32 + 1);
    let (z0, z1) = (slab.min[2].floor() as i32 - 1, slab.max[2].ceil() as i32 + 1);
    let (mut sum, mut n) = (0.0f32, 0i32);
    let mut cell = Vec::new();
    for x in x0..x1 {
        for z in z0..z1 {
            let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
            if edges == 2 {
                continue;
            }
            for y in y0..y1 {
                if edges > 0 && (y == y0 || y == y1 - 1) {
                    continue;
                }
                let s = w.state(x, y, z).unwrap_or(0);
                if t.lily_pad(s) {
                    continue;
                }
                cell.clear();
                w.collision(x, y, z, &mut cell);
                if cell.iter().any(|c| c.offset(f64::from(x), f64::from(y), f64::from(z)).intersects(&slab)) {
                    sum += t.friction.get(s as usize).copied().unwrap_or(0.6);
                    n += 1;
                }
            }
        }
    }
    sum / n as f32
}
