//! Minecart movement, "old" behavior of game 26.3 (world without the
//! `minecart_improvements` experiment): gravity, rail following (slopes, junctions, propulsion
//! and stop rails), leaving the rail with collisions, slowdown, rotation and flipping,
//! pushes between minecarts and boarding of mobs, fluids, lava.

use rc_physics::collide;

use super::{CKind, Cart, WIDTH};
use crate::mobs::mob::Ent;
use crate::mobs::mth::{atan2, wrap};
use crate::mobs::recent::vibration::Src;
use crate::server::Server;

/// Shape of a rail and its two exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    NorthSouth,
    EastWest,
    AscendingEast,
    AscendingWest,
    AscendingNorth,
    AscendingSouth,
    SouthEast,
    SouthWest,
    NorthWest,
    NorthEast,
}

impl Shape {
    fn parse(s: &str) -> Option<Shape> {
        Some(match s {
            "north_south" => Shape::NorthSouth,
            "east_west" => Shape::EastWest,
            "ascending_east" => Shape::AscendingEast,
            "ascending_west" => Shape::AscendingWest,
            "ascending_north" => Shape::AscendingNorth,
            "ascending_south" => Shape::AscendingSouth,
            "south_east" => Shape::SouthEast,
            "south_west" => Shape::SouthWest,
            "north_west" => Shape::NorthWest,
            "north_east" => Shape::NorthEast,
            _ => return None,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_slope(self) -> bool {
        matches!(self, Shape::AscendingEast | Shape::AscendingWest | Shape::AscendingNorth | Shape::AscendingSouth)
    }

    /// The two exits (x, y, z) of the rail.
    pub(crate) fn exits(self) -> ([i32; 3], [i32; 3]) {
        const W: [i32; 3] = [-1, 0, 0];
        const E: [i32; 3] = [1, 0, 0];
        const N: [i32; 3] = [0, 0, -1];
        const S: [i32; 3] = [0, 0, 1];
        let below = |v: [i32; 3]| [v[0], v[1] - 1, v[2]];
        match self {
            Shape::NorthSouth => (N, S),
            Shape::EastWest => (W, E),
            Shape::AscendingEast => (below(W), E),
            Shape::AscendingWest => (W, below(E)),
            Shape::AscendingNorth => (N, below(S)),
            Shape::AscendingSouth => (below(N), S),
            Shape::SouthEast => (S, E),
            Shape::SouthWest => (S, W),
            Shape::NorthWest => (N, W),
            Shape::NorthEast => (N, E),
        }
    }
}

fn l2(v: [f64; 3]) -> f64 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

fn hdist(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[2] * v[2]).sqrt()
}

/// Equality within a tolerance (difference under 1e-5).
fn equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}

impl Server {
    /// Rail (tag `rails` and shape property): its shape.
    pub(crate) fn rail_shape_at(&self, p: [i32; 3]) -> Option<Shape> {
        let st = self.st(p);
        if !self.btag_id(p, crate::tag_bits::TagId::Rails) {
            return None;
        }
        Shape::parse(self.v().get(st, "shape")?)
    }

    /// The position of the current block or of the rail below (old behavior): a rail just below counts.
    pub(crate) fn cart_rail_pos(&self, pos: [f64; 3]) -> [i32; 3] {
        let (x, mut y, z) = (pos[0].floor() as i32, pos[1].floor() as i32, pos[2].floor() as i32);
        if self.btag_id([x, y - 1, z], crate::tag_bits::TagId::Rails) {
            y -= 1;
        }
        [x, y, z]
    }

    /// Point of the rail under `(x, y, z)`, slope height included.
    pub(crate) fn cart_track_pos(&self, x: f64, y: f64, z: f64) -> Option<[f64; 3]> {
        let p = self.cart_rail_pos([x, y, z]);
        let shape = self.rail_shape_at(p)?;
        let (e0, e1) = shape.exits();
        let (xt, yt, zt) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
        let x0 = xt + 0.5 + f64::from(e0[0]) * 0.5;
        let y0 = yt + 0.0625 + f64::from(e0[1]) * 0.5;
        let z0 = zt + 0.5 + f64::from(e0[2]) * 0.5;
        let x1 = xt + 0.5 + f64::from(e1[0]) * 0.5;
        let y1 = yt + 0.0625 + f64::from(e1[1]) * 0.5;
        let z1 = zt + 0.5 + f64::from(e1[2]) * 0.5;
        let xd = x1 - x0;
        let yd = (y1 - y0) * 2.0;
        let zd = z1 - z0;
        let progress = if xd == 0.0 {
            z - zt
        } else if zd == 0.0 {
            x - xt
        } else {
            ((x - x0) * xd + (z - z0) * zd) * 2.0
        };
        let mut ry = y0 + yd * progress;
        if yd < 0.0 {
            ry += 1.0;
        } else if yd > 0.0 {
            ry += 0.5;
        }
        Some([x0 + xd * progress, ry, z0 + zd * progress])
    }

    /// Redstone conductive block.
    fn cart_conductor(&self, p: [i32; 3]) -> bool {
        self.v().conductor(self.st(p))
    }

    /// Maximum speed: 0.4, 0.2 in water; furnace × 0.5 (× 0.75 in water).
    pub(crate) fn cart_max_speed(c: &Cart) -> f64 {
        let base = if c.in_water { 0.2 } else { 0.4 };
        match c.kind {
            CKind::Furnace if c.in_water => base * 0.75,
            CKind::Furnace => base * 0.5,
            _ => base,
        }
    }

    /// One minecart tick (then that of the kind).
    pub(crate) fn cart_tick(&mut self, c: &mut Cart) {
        c.consumed = false;
        if c.hurt_time > 0 {
            c.hurt_time -= 1;
        }
        if c.damage > 0.0 {
            c.damage -= 1.0;
        }
        c.sync_hurt();
        // Below the world: removed (the contents drop, like any destruction).
        if c.pos[1] < f64::from(self.dim.height.min_y - 64) {
            self.cart_discard(c);
            return;
        }
        self.cart_behavior_tick(c);
        // Removed during the movement (fall of a TNT minecart, break): the end of the tick
        // continues as in the game (fluids, lava, kind: second explosion possible).
        self.cart_fluids(c);
        if c.in_lava {
            // Setting on fire by lava then lava damage: fire of 15 s, 4 lava damage.
            c.fire = c.fire.max(300);
            self.cart_set_frozen(c, 0);
            self.cart_lava_hurt(c);
            c.fall_distance *= 0.5;
        }
        c.first_tick = false;
        self.cart_variant_tick(c);
    }

    /// The minecart tick on the server side.
    fn cart_behavior_tick(&mut self, c: &mut Cart) {
        if !c.no_gravity {
            c.vel[1] -= if c.in_water { 0.005 } else { 0.04 };
        }
        let bp = self.cart_rail_pos(c.pos);
        let shape = self.rail_shape_at(bp);
        c.on_rails = shape.is_some();
        if let Some(shape) = shape {
            self.cart_move_along_track(c, bp, shape);
            if self.bname(bp) == "minecraft:activator_rail" {
                let powered = self.v().get(self.st(bp), "powered") == Some("true");
                self.cart_activate(c, powered);
            }
        } else {
            self.cart_come_off_track(c);
        }
        if c.removed {
            return;
        }
        self.cart_block_effects(c);
        c.pitch = 0.0;
        let xd = c.old_pos[0] - c.pos[0];
        let zd = c.old_pos[2] - c.pos[2];
        if xd * xd + zd * zd > 0.001 {
            c.yaw = (atan2(zd, xd) * 180.0 / std::f64::consts::PI) as f32;
            if c.flipped {
                c.yaw += 180.0;
            }
        }
        let rd = f64::from(wrap(c.yaw - c.yaw_o));
        if !(-170.0..170.0).contains(&rd) {
            c.yaw += 180.0;
            c.flipped = !c.flipped;
        }
        c.pitch %= 360.0;
        c.yaw %= 360.0;
        self.cart_push_and_pickup(c);
    }

    /// The movement of the minecart along the rail.
    fn cart_move_along_track(&mut self, c: &mut Cart, p: [i32; 3], shape: Shape) {
        c.fall_distance = 0.0;
        let (mut x, _, mut z) = (c.pos[0], c.pos[1], c.pos[2]);
        let old = self.cart_track_pos(c.pos[0], c.pos[1], c.pos[2]);
        let mut y = f64::from(p[1]);
        let name = self.bname(p).to_owned();
        let (mut power, mut halt) = (false, false);
        if name == "minecraft:powered_rail" {
            power = self.v().get(self.st(p), "powered") == Some("true");
            halt = !power;
        }
        let mut slide = 0.007_812_5;
        if c.in_water {
            slide *= 0.2;
        }
        match shape {
            Shape::AscendingEast => {
                c.vel[0] -= slide;
                y += 1.0;
            }
            Shape::AscendingWest => {
                c.vel[0] += slide;
                y += 1.0;
            }
            Shape::AscendingNorth => {
                c.vel[2] += slide;
                y += 1.0;
            }
            Shape::AscendingSouth => {
                c.vel[2] -= slide;
                y += 1.0;
            }
            _ => {}
        }
        let (e0, e1) = shape.exits();
        let mut xd = f64::from(e1[0] - e0[0]);
        let mut zd = f64::from(e1[2] - e0[2]);
        let length = (xd * xd + zd * zd).sqrt();
        if c.vel[0] * xd + c.vel[2] * zd < 0.0 {
            xd = -xd;
            zd = -zd;
        }
        let pow = hdist(c.vel).min(2.0);
        c.vel = [pow * xd / length, c.vel[1], pow * zd / length];
        // Player passenger: their movement intention restarts a nearly stopped minecart.
        if c.passengers.first() == Some(&Ent::Player) {
            let intent = self.player_move_intent();
            if l2(intent) > 0.0 {
                let own = c.vel[0] * c.vel[0] + c.vel[2] * c.vel[2];
                if own < 0.01 {
                    c.vel[0] += intent[0] * 0.001;
                    c.vel[2] += intent[2] * 0.001;
                    halt = false;
                }
            }
        }
        if halt {
            if hdist(c.vel) < 0.03 {
                c.vel = [0.0; 3];
            } else {
                c.vel = [c.vel[0] * 0.5, 0.0, c.vel[2] * 0.5];
            }
        }
        let (px, pz) = (f64::from(p[0]), f64::from(p[2]));
        let x0 = px + 0.5 + f64::from(e0[0]) * 0.5;
        let z0 = pz + 0.5 + f64::from(e0[2]) * 0.5;
        let x1 = px + 0.5 + f64::from(e1[0]) * 0.5;
        let z1 = pz + 0.5 + f64::from(e1[2]) * 0.5;
        let xd = x1 - x0;
        let zd = z1 - z0;
        let progress = if xd == 0.0 {
            z - pz
        } else if zd == 0.0 {
            x - px
        } else {
            ((x - x0) * xd + (z - z0) * zd) * 2.0
        };
        x = x0 + xd * progress;
        z = z0 + zd * progress;
        c.pos = [x, y, z];
        let scale = if c.is_vehicle() { 0.75 } else { 1.0 };
        let max = Self::cart_max_speed(c);
        let d = [(scale * c.vel[0]).clamp(-max, max), 0.0, (scale * c.vel[2]).clamp(-max, max)];
        self.cart_move(c, d);
        if c.removed {
            return;
        }
        let (fx, fz) = (c.pos[0].floor() as i32 - p[0], c.pos[2].floor() as i32 - p[2]);
        if e0[1] != 0 && fx == e0[0] && fz == e0[2] {
            c.pos[1] += f64::from(e0[1]);
        } else if e1[1] != 0 && fx == e1[0] && fz == e1[2] {
            c.pos[1] += f64::from(e1[1]);
        }
        c.vel = self.cart_natural_slowdown(c);
        let new = self.cart_track_pos(c.pos[0], c.pos[1], c.pos[2]);
        if let (Some(n), Some(o)) = (new, old) {
            let speed = (o[1] - n[1]) * 0.05;
            let other = hdist(c.vel);
            if other > 0.0 {
                let k = (other + speed) / other;
                c.vel = [c.vel[0] * k, c.vel[1], c.vel[2] * k];
            }
            c.pos[1] = n[1];
        }
        let (xn, zn) = (c.pos[0].floor() as i32, c.pos[2].floor() as i32);
        if xn != p[0] || zn != p[2] {
            let other = hdist(c.vel);
            c.vel = [other * f64::from(xn - p[0]), c.vel[1], other * f64::from(zn - p[2])];
        }
        if power {
            let speed = hdist(c.vel);
            if speed > 0.01 {
                c.vel = [c.vel[0] + c.vel[0] / speed * 0.06, c.vel[1], c.vel[2] + c.vel[2] / speed * 0.06];
            } else {
                let (mut dx, mut dz) = (c.vel[0], c.vel[2]);
                match shape {
                    Shape::EastWest => {
                        if self.cart_conductor([p[0] - 1, p[1], p[2]]) {
                            dx = 0.02;
                        } else if self.cart_conductor([p[0] + 1, p[1], p[2]]) {
                            dx = -0.02;
                        }
                    }
                    Shape::NorthSouth => {
                        if self.cart_conductor([p[0], p[1], p[2] - 1]) {
                            dz = 0.02;
                        } else if self.cart_conductor([p[0], p[1], p[2] + 1]) {
                            dz = -0.02;
                        }
                    }
                    _ => return,
                }
                c.vel = [dx, c.vel[1], dz];
            }
        }
    }

    /// Bounded speed, braked on the ground, movement, drag in the air.
    fn cart_come_off_track(&mut self, c: &mut Cart) {
        let max = Self::cart_max_speed(c);
        c.vel = [c.vel[0].clamp(-max, max), c.vel[1], c.vel[2].clamp(-max, max)];
        if c.on_ground {
            c.vel = c.vel.map(|v| v * 0.5);
        }
        let d = c.vel;
        self.cart_move(c, d);
        if !c.on_ground {
            let drag = f64::from(0.95f32);
            c.vel = c.vel.map(|v| v * drag);
        }
    }

    /// Natural slowdown (of the kind).
    fn cart_natural_slowdown(&mut self, c: &mut Cart) -> [f64; 3] {
        let v = c.vel;
        match c.kind {
            CKind::Chest | CKind::Hopper => {
                let mut keep = 0.98f32;
                if c.loot.is_none() {
                    let empty = 15 - self.cart_signal(c);
                    keep += empty as f32 * 0.001;
                }
                if c.in_water {
                    keep *= 0.95;
                }
                let k = f64::from(keep);
                [v[0] * k, 0.0, v[2] * k]
            }
            CKind::Furnace => {
                let nv = self.furnace_slowdown(c, v);
                self.base_slowdown(c, nv)
            }
            _ => self.base_slowdown(c, v),
        }
    }

    /// 0.997 occupied, 0.96 otherwise; water × 0.95.
    pub(crate) fn base_slowdown(&self, c: &Cart, v: [f64; 3]) -> [f64; 3] {
        let f = if c.is_vehicle() { 0.997 } else { 0.96 };
        let mut n = [v[0] * f, 0.0, v[2] * f];
        if c.in_water {
            let w = f64::from(0.95f32);
            n = n.map(|x| x * w);
        }
        n
    }

    /// Fill signal of the container.
    pub(crate) fn cart_signal(&self, c: &Cart) -> i32 {
        if c.items.is_empty() {
            return 0;
        }
        let mut r = 0.0f32;
        for s in c.items.iter().flatten() {
            r += s.count as f32 / self.life.items.max_stack(s).max(1) as f32;
        }
        r /= c.items.len() as f32;
        (r * 14.0).floor() as i32 + i32::from(r > 0.0)
    }

    /// The movement of an entity with the collisions of a minecart, then effects of the blocks passed through.
    pub(crate) fn cart_move(&mut self, c: &mut Cart, mut delta: [f64; 3]) {
        // Stuck in the previous tick: reduced movement, velocity reset to zero.
        if l2(c.stuck) > 1.0e-7 {
            delta = std::array::from_fn(|i| delta[i] * c.stuck[i]);
            c.stuck = [0.0; 3];
            c.vel = [0.0; 3];
        }
        self.cart_move_raw(c, delta);
        self.cart_block_effects(c);
    }

    fn cart_move_raw(&mut self, c: &mut Cart, delta: [f64; 3]) {
        let start = c.bbox();
        let mv = collide::resolve(&self.world(), &start, delta, c.on_ground, 0.0);
        let ml = l2(mv);
        if ml > 1.0e-7 || l2(delta) - ml < 1.0e-7 {
            for (p, v) in c.pos.iter_mut().zip(mv) {
                *p += v;
            }
        }
        let xc = !equal(delta[0], mv[0]);
        let zc = !equal(delta[2], mv[2]);
        c.horizontal_collision = xc || zc;
        // No player drives a minecart: the server is always authoritative (ground updated even
        // without vertical movement).
        c.vertical_collision = delta[1] != mv[1];
        let below = c.vertical_collision && delta[1] < 0.0;
        c.on_ground = below;
        c.support = if below { rc_physics::support_of(&self.world(), &c.bbox(), c.pos) } else { None };
        let effect = self.cart_on_pos(c, 0.2);
        let effect_st = self.st(effect);
        // The fall damage check.
        if !c.in_water && mv[1] < 0.0 {
            c.fall_distance -= f64::from(mv[1] as f32);
        }
        if c.on_ground {
            if c.fall_distance > 0.0 {
                self.cart_fall_on(c, effect);
                let st = self.st(c.support.unwrap_or(effect));
                self.game_event_src("minecraft:hit_ground", c.pos, Some(Src::Other), Some(st));
            }
            c.fall_distance = 0.0;
        }
        if c.removed {
            return;
        }
        // Bounce: no bounce of its own; bouncing block
        // under the minecart × 0.8 (non-living entity).
        if (delta[1].abs() > 0.0 && c.vertical_collision) || c.horizontal_collision {
            let cur = c.vel;
            let mut v = cur;
            if xc {
                v[0] = -cur[0] * 0.0;
            }
            if zc {
                v[2] = -cur[2] * 0.0;
            }
            let mut bounced = false;
            if c.vertical_collision {
                let gravity = if c.no_gravity {
                    0.0
                } else if c.in_water {
                    0.005
                } else {
                    0.04
                };
                let mut r = 0.0;
                if below && -cur[1] > gravity && !self.btag_id(effect, crate::tag_bits::TagId::SuppressesBounce) {
                    let b = self.phys.bounce.get(effect_st as usize).copied().unwrap_or(0.0) * 0.8;
                    r = f64::from(b).max(0.0);
                }
                v[1] = if r > 0.0 {
                    bounced = true;
                    let portion = mv[1] / cur[1];
                    let comp = portion * gravity;
                    let drag = 1.0 + (f64::from(0.95f32) - 1.0) * portion;
                    (comp - cur[1]) * drag * r
                } else {
                    -cur[1] * 0.0
                };
            }
            if bounced {
                self.game_event_src("minecraft:bounce", c.pos, Some(Src::Other), None);
            }
            c.vel = v;
        }
        // Movement emission (`EVENTS`): not on the rail or on the ground, without sound.
        if !self.cart_is_passenger(c) {
            self.cart_movement_events(c, mv, effect, effect_st);
        }
        // Speed factor: 1 on a rail.
        let here = c.block_pos();
        let f = if self.btag_id(here, crate::tag_bits::TagId::Rails) { 1.0 } else { self.cart_speed_factor(c) };
        c.vel[0] *= f64::from(f);
        c.vel[2] *= f64::from(f);
    }

    /// The speed factor of the off-rail block (honey, soul sand): block present, otherwise below.
    fn cart_speed_factor(&self, c: &Cart) -> f32 {
        let s = self.st(c.block_pos());
        let f = self.phys.speed_factor.get(s as usize).copied().unwrap_or(1.0);
        let name = self.state_name(s);
        if name == "minecraft:water" || name == "minecraft:bubble_column" || f != 1.0 {
            return f;
        }
        let b = self.st(self.cart_on_pos(c, 0.500_001));
        self.phys.speed_factor.get(b as usize).copied().unwrap_or(1.0)
    }

    /// Main supporting block, otherwise under the feet.
    pub(crate) fn cart_on_pos(&self, c: &Cart, off: f32) -> [i32; 3] {
        self.on_pos(c.pos, c.support, off)
    }

    /// Step events (emission `EVENTS`).
    fn cart_movement_events(&mut self, c: &mut Cart, mv: [f64; 3], effect: [i32; 3], effect_st: u32) {
        let on = self.cart_on_pos(c, 1.0e-5);
        let on_st = self.st(on);
        let climbing = self.climbable_state(on_st);
        let full = l2(mv).sqrt();
        let h = (mv[0] * mv[0] + mv[2] * mv[2]).sqrt();
        c.move_dist += if climbing { (full * f64::from(0.6f32)) as f32 } else { (h * f64::from(0.6f32)) as f32 };
        if c.move_dist > c.next_step && !self.v().is_air(on_st) {
            let same = on == effect;
            let mut produced = self.cart_step_event(c, effect, effect_st, same);
            if !same {
                produced |= self.cart_step_event(c, on, on_st, true);
            }
            if produced {
                c.next_step = c.move_dist.trunc() + 1.0;
            } else if c.in_water {
                c.next_step = c.move_dist.trunc() + 1.0;
                self.game_event_src("minecraft:swim", c.pos, Some(Src::Other), None);
            }
        }
    }

    fn cart_step_event(&mut self, c: &Cart, _p: [i32; 3], st: u32, vibrate: bool) -> bool {
        if self.v().is_air(st) {
            return false;
        }
        let climb = self.climbable_state(st);
        if !(c.on_ground || climb || c.on_rails) {
            return false;
        }
        if vibrate {
            self.game_event_src("minecraft:step", c.pos, Some(Src::Other), Some(st));
        }
        true
    }

    /// Fall onto a block (reaction of the touched block, then fall damage): distance and factor
    /// according to the block (pointed dripstone tip + 2.5 ×2, bed and shelf fungus ÷ 2, hay and
    /// honey × 0.2, slime × 0, powder snow nothing; farmland draws from the world's random draw,
    /// honey plays its slide), then the TNT minecart explodes from 3 blocks and the
    /// passengers take the fall.
    fn cart_fall_on(&mut self, c: &mut Cart, p: [i32; 3]) {
        let st = self.st(p);
        let mut dist = c.fall_distance;
        let mut mult = 1.0;
        let mut ty = "minecraft:fall";
        match self.bname(p) {
            "minecraft:powder_snow" => return,
            "minecraft:pointed_dripstone"
                if self.v().get(st, "vertical_direction") == Some("up") && self.v().get(st, "thickness") == Some("tip") =>
            {
                dist += 2.5;
                mult = 2.0;
                ty = "minecraft:stalagmite";
            }
            "minecraft:farmland" => {
                let _ = self.bs.rng.next_float();
            }
            "minecraft:hay_block" => mult = 0.2,
            "minecraft:honey_block" => {
                if !c.silent {
                    self.world_sound("minecraft:block.honey_block.slide", 6, c.pos, 1.0, 1.0);
                }
                self.cart_event(c, 54);
                mult = 0.2;
            }
            "minecraft:slime_block" => mult = 0.0,
            "minecraft:shelf_mushroom" => dist *= 0.5,
            _ if self.btag_id(p, crate::tag_bits::TagId::Beds) => dist *= 0.5,
            _ => {}
        }
        if c.kind == CKind::Tnt && dist >= 3.0 {
            let power = dist / 10.0;
            let by = c.ignition.flatten();
            self.tnt_cart_explode(c, by, power * power);
        }
        if !c.passengers.is_empty() {
            self.propagate_fall_to(&c.passengers.clone(), dist, mult, ty);
        }
    }

    /// Effects of the blocks at the position (after each movement and once in the tick):
    /// block stepped on the ground (lit redstone ore), then blocks touched in x, y, z order
    /// (plates, tripwire, detector rail, cobweb and powder snow that entangle, cactus), then the
    /// deferred effects in the game's order: freezing, fire (ignition then damage of each fire),
    /// lava cauldron, water and powder snow cauldrons that extinguish; finally the fire state
    /// (rain, extinguishing sound, −1 off fire). Not reproduced: bubble column, honey,
    /// turtle eggs, stepped-on sculk sensor, big dripleaf (docs/gaps-client.md).
    pub(crate) fn cart_block_effects(&mut self, c: &mut Cart) {
        if c.removed {
            return;
        }
        if c.on_ground {
            let p = self.cart_on_pos(c, 0.2);
            if matches!(self.bname(p), "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore") {
                let s = self.st(p);
                if !self.v().get_bool(s, "lit") {
                    let ns = self.v().set_bool(s, "lit", true);
                    self.set_block(p, ns, crate::blocks::flag::ALL);
                }
            }
        }
        let was_on_fire = c.fire > 0;
        let before = c.fire;
        let b = c.bbox().deflate(1.0e-5);
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.floor() as i32);
        // Deferred effects: freezing, fire damage, lava cauldrons, extinguishing (block, state).
        let mut freeze = false;
        let mut fires: Vec<f32> = Vec::new();
        let mut lava = 0;
        let mut quench: Vec<([i32; 3], bool)> = Vec::new();
        self.carts.ticking = Some(c.pos);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let p = [x, y, z];
                    match self.bname(p) {
                        n if n.ends_with("pressure_plate") => self.plate_entity_inside(p),
                        "minecraft:tripwire" => self.tripwire_entity_inside(p),
                        "minecraft:detector_rail" => self.detector_rail_inside(p),
                        "minecraft:cobweb" => {
                            c.fall_distance = 0.0;
                            c.stuck = [0.25, f64::from(0.05f32), 0.25];
                        }
                        "minecraft:powder_snow" => {
                            c.fall_distance = 0.0;
                            c.stuck = [f64::from(0.9f32), 1.5, f64::from(0.9f32)];
                            // Snowflakes when it moves (world's random draw).
                            if (c.vel[0] != 0.0 || c.vel[2] != 0.0) && self.bs.rng.next_int_bound(2) != 0 {
                                let k = 0.083_333_336f32;
                                let sx = (self.bs.rng.next_float() * 2.0 - 1.0) * k;
                                let sz = (self.bs.rng.next_float() * 2.0 - 1.0) * k;
                                let at = [c.pos[0], f64::from(y + 1), c.pos[2]];
                                self.particles_full("minecraft:snowflake", 0, at, [0.0; 3], [sx, 0.05, sz], 1, 1);
                            }
                            quench.push((p, true));
                            freeze = true;
                        }
                        "minecraft:cactus" => {
                            self.cart_hurt(c, "minecraft:cactus", 1.0, None);
                        }
                        "minecraft:fire" => fires.push(1.0),
                        "minecraft:soul_fire" => fires.push(2.0),
                        "minecraft:lava_cauldron" => lava += 1,
                        "minecraft:water_cauldron" | "minecraft:powder_snow_cauldron" => quench.push((p, false)),
                        _ => {}
                    }
                }
            }
        }
        self.carts.ticking = None;
        // Freezing can only stop through fire or lava: read only in that case.
        let thaws = !fires.is_empty() || lava > 0;
        let was_freezing = thaws && crate::nbtio::int(&c.nbt, "TicksFrozen").unwrap_or(0) > 0;
        self.cart_inside_effects(c, freeze, &fires, lava, &quench);
        // End of effects: rain, extinguishing sound, 0 off fire (only the player keeps −20).
        if !c.removed {
            let top = [c.pos[0].floor() as i32, c.bbox().max[1].floor() as i32, c.pos[2].floor() as i32];
            // Rain only extinguishes a burning minecart (otherwise nothing changes): tests skipped.
            if c.fire > 0 && (self.is_raining_at(c.block_pos()) || self.is_raining_at(top)) {
                c.fire = c.fire.min(0);
            }
            let freezing = thaws && crate::nbtio::int(&c.nbt, "TicksFrozen").unwrap_or(0) > 0;
            if (was_on_fire && c.fire <= 0) || (was_freezing && !freezing) {
                let pitch = 1.6 + (c.rng.next_float() - c.rng.next_float()) * 0.4;
                if !c.silent {
                    self.world_sound("minecraft:entity.generic.extinguish_fire", 6, c.pos, 0.7, pitch);
                }
            }
            if c.fire <= 0 && c.fire <= before {
                c.fire = 0;
            }
        }
    }

    /// Deferred effects of the touched blocks, in the game's order (stop as soon as the minecart is
    /// removed): freezing, ignition then damage of each fire, lava (ignition then damage of each
    /// cauldron), cauldrons and powder snow emptied if the minecart burns, extinguishing.
    fn cart_inside_effects(&mut self, c: &mut Cart, freeze: bool, fires: &[f32], lava: i32, quench: &[([i32; 3], bool)]) {
        if c.removed {
            return;
        }
        if freeze {
            let t = crate::nbtio::int(&c.nbt, "TicksFrozen").unwrap_or(0) as i32;
            self.cart_set_frozen(c, (t + 1).min(140));
        }
        if !fires.is_empty() {
            // Always "thaw" with fire; a negative fire goes up by one, ignition 8 s from 0.
            self.cart_set_frozen(c, 0);
            if c.fire < 0 {
                c.fire += 1;
            }
            if c.fire >= 0 && c.fire < 160 {
                c.fire = 160;
            }
            for &d in fires {
                if c.removed {
                    return;
                }
                self.cart_hurt(c, "minecraft:in_fire", d, None);
            }
        }
        if lava > 0 {
            if c.removed {
                return;
            }
            self.cart_set_frozen(c, 0);
            if c.fire < 300 {
                c.fire = 300;
            }
            for _ in 0..lava {
                if c.removed {
                    return;
                }
                self.cart_lava_hurt(c);
            }
        }
        if !quench.is_empty() {
            for &(p, snow) in quench {
                if c.removed {
                    return;
                }
                if c.fire > 0 {
                    if snow {
                        if self.rule("minecraft:mob_griefing", true) {
                            self.destroy_block(p, false, 512);
                        }
                    } else {
                        let s = self.st(p);
                        let s = if self.bname(p) == "minecraft:powder_snow_cauldron" {
                            let w = self.default_state("minecraft:water_cauldron");
                            self.v().set_int(w, "level", self.v().get_int(s, "level"))
                        } else {
                            s
                        };
                        self.cauldron_lower(p, s);
                    }
                }
            }
            if !c.removed {
                c.fire = c.fire.min(0);
            }
        }
    }

    /// Lava damage (4) and burning sound if the hit lands.
    pub(crate) fn cart_lava_hurt(&mut self, c: &mut Cart) {
        // An already removed minecart "takes" the hit (sound included), as in the game.
        let hit = self.cart_hurt(c, "minecraft:lava", 4.0, None);
        if hit && !c.silent {
            let pitch = 2.0 + c.rng.next_float() * 0.4;
            self.world_sound("minecraft:entity.generic.burn", 6, c.pos, 0.4, pitch);
        }
    }

    /// Freezing ticks (kept field and synchronized data 7).
    fn cart_set_frozen(&mut self, c: &mut Cart, t: i32) {
        let old = crate::nbtio::int(&c.nbt, "TicksFrozen").unwrap_or(0) as i32;
        if old == t {
            return;
        }
        if t == 0 {
            crate::nbtio::remove(&mut c.nbt, "TicksFrozen");
        } else {
            crate::nbtio::set(&mut c.nbt, "TicksFrozen", rc_proto::nbt::Nbt::Int(t));
        }
        c.set_meta(7, rc_proto_v777::entity::Meta::Int(t));
    }

    /// Fluids: water (fall reset, splash, current), lava.
    fn cart_fluids(&mut self, c: &mut Cart) {
        let eye = c.pos[1] + super::HEIGHT * 0.85;
        let mut flow = c.vel;
        let w = self.world();
        let (water, lava, _) =
            rc_physics::scan_fluids_entity(&w, &self.phys, &c.bbox(), c.pos, eye, true, self.fast_lava(), &mut flow);
        c.vel = flow;
        let in_water = water > 0.0;
        if in_water {
            c.fall_distance = 0.0;
            if !c.in_water && !c.first_tick {
                self.cart_splash(c);
            }
        }
        c.in_water = in_water;
        c.in_lava = !c.first_tick && lava > 0.0;
        if in_water {
            c.fire = c.fire.min(0);
        }
    }

    /// The splash in the water of a non-living entity (volume × 0.2); the server
    /// particles are not sent, the random draw is consumed like the game.
    fn cart_splash(&mut self, c: &mut Cart) {
        let v = c.vel;
        let speed = ((v[0] * v[0] * f64::from(0.2f32) + v[1] * v[1] + v[2] * v[2] * f64::from(0.2f32)).sqrt() as f32
            * 0.2)
            .min(1.0);
        let pitch = 1.0 + (c.rng.next_float() - c.rng.next_float()) * 0.4;
        if !c.silent {
            self.world_sound("minecraft:entity.generic.splash", 6, c.pos, speed, pitch);
        }
        let n = (1.0 + WIDTH as f32 * 20.0).ceil() as i32;
        for _ in 0..n {
            c.rng.next_double();
            c.rng.next_double();
            c.rng.next_double();
        }
        for _ in 0..n {
            c.rng.next_double();
            c.rng.next_double();
        }
        self.game_event_src("minecraft:splash", c.pos, Some(Src::Other), None);
    }

    /// A simple minecart that rolls boards the touched mobs
    /// (except iron golems), pushes player and golems; otherwise it only pushes minecarts.
    fn cart_push_and_pickup(&mut self, c: &mut Cart) {
        let b = c.bbox();
        let hit = rc_physics::Aabb {
            min: [b.min[0] - f64::from(0.2f32), b.min[1], b.min[2] - f64::from(0.2f32)],
            max: [b.max[0] + f64::from(0.2f32), b.max[1], b.max[2] + f64::from(0.2f32)],
        };
        let rideable = c.kind == CKind::Rideable;
        if rideable && c.vel[0] * c.vel[0] + c.vel[2] * c.vel[2] >= 0.01 {
            // Pushable mobs touched, in list order.
            let ids: Vec<i32> = self
                .mobs
                .list
                .iter()
                .flatten()
                .filter(|m| m.alive() && m.touches(&hit) && !c.passengers.contains(&Ent::Mob(m.id)))
                .filter(|m| self.mob_pushable(m))
                .map(|m| m.id)
                .collect();
            for id in ids {
                let Some(i) = self.mob_index(id) else { continue };
                let Some(mut m) = self.mobs.list[i].take() else { continue };
                let golem = m.kind == crate::mobs::species::Kind::IronGolem;
                if !golem && !c.is_vehicle() && m.mount.vehicle.is_none() {
                    self.cart_mob_start_riding(c, &mut m);
                } else {
                    self.cart_push_mob(c, &mut m);
                }
                self.mobs.list[i] = Some(m);
            }
            // The player is pushed (never boarded).
            if self.cart_player_pushable(c) && self.player_aabb().intersects(&hit) {
                self.cart_push_player(c);
            }
            // Other minecarts touched: pushed by this one.
            for i in 0..self.carts.list.len() {
                let Some(mut o) = self.carts.list[i].take() else { continue };
                if !o.removed && o.bbox().intersects(&hit) {
                    Self::cart_push_cart(&mut o, c);
                }
                self.carts.list[i] = Some(o);
            }
        } else {
            for i in 0..self.carts.list.len() {
                let Some(mut o) = self.carts.list[i].take() else { continue };
                if !o.removed && o.bbox().intersects(&hit) {
                    Self::cart_push_cart(&mut o, c);
                }
                self.carts.list[i] = Some(o);
            }
        }
    }

    /// Push between minecarts: normalized difference, × min(1, 1/d) × 0.1 × 0.5;
    /// `None` if the two coincide.
    pub(crate) fn cart_push_amount(me: [f64; 3], other: [f64; 3]) -> Option<(f64, f64)> {
        let xa = other[0] - me[0];
        let za = other[2] - me[2];
        let dd = xa * xa + za * za;
        if dd < f64::from(1.0e-4f32) {
            return None;
        }
        let dd = dd.sqrt();
        let pow = (1.0 / dd).min(1.0);
        let k = pow * f64::from(0.1f32) * 0.5;
        Some((xa / dd * k, za / dd * k))
    }

    /// `me.push(by)` when both are minecarts.
    fn cart_push_cart(me: &mut Cart, by: &mut Cart) {
        if let Some((xa, za)) = Self::cart_push_amount(me.pos, by.pos) {
            Self::push_other_minecart(me, by, xa, za);
        }
    }

    /// `cart.push(e)` against an entity that is not a minecart: the minecart moves aside, the other
    /// receives the quarter (given back to the caller).
    pub(crate) fn cart_pushed_by(c: &mut Cart, other: [f64; 3]) -> Option<(f64, f64)> {
        let (xa, za) = Self::cart_push_amount(c.pos, other)?;
        Self::cart_push_vel(c, -xa, -za);
        Some((xa / 4.0, za / 4.0))
    }

    /// The push of another minecart (old behavior: only if the other is in the axis).
    fn push_other_minecart(me: &mut Cart, other: &mut Cart, xa: f64, za: f64) {
        let (xo, zo) = (other.pos[0] - me.pos[0], other.pos[2] - me.pos[2]);
        let l = (xo * xo + zo * zo).sqrt();
        let dir = if l < 1.0e-4 { [0.0, 0.0] } else { [xo / l, zo / l] };
        let r = me.yaw * crate::mobs::mth::DEG;
        let (fc, fs) = (f64::from(crate::mobs::mth::cos(r)), f64::from(crate::mobs::mth::sin(r)));
        let fl = (fc * fc + fs * fs).sqrt();
        let facing = if fl < 1.0e-4 { [0.0, 0.0] } else { [fc / fl, fs / fl] };
        let dot = (dir[0] * facing[0] + dir[1] * facing[1]).abs();
        if dot < f64::from(0.8f32) {
            return;
        }
        let m = me.vel;
        let e = other.vel;
        let me_f = me.kind == CKind::Furnace;
        let other_f = other.kind == CKind::Furnace;
        if other_f && !me_f {
            me.vel = [m[0] * 0.2, m[1], m[2] * 0.2];
            Self::cart_push_vel(me, e[0] - xa, e[2] - za);
            other.vel = [e[0] * 0.95, e[1], e[2] * 0.95];
        } else if !other_f && me_f {
            other.vel = [e[0] * 0.2, e[1], e[2] * 0.2];
            Self::cart_push_vel(other, m[0] + xa, m[2] + za);
            me.vel = [m[0] * 0.95, m[1], m[2] * 0.95];
        } else {
            let xdd = (e[0] + m[0]) / 2.0;
            let zdd = (e[2] + m[2]) / 2.0;
            me.vel = [m[0] * 0.2, m[1], m[2] * 0.2];
            Self::cart_push_vel(me, xdd - xa, zdd - za);
            other.vel = [e[0] * 0.2, e[1], e[2] * 0.2];
            Self::cart_push_vel(other, xdd + xa, zdd + za);
        }
    }

    /// The addition of velocity to an entity (added velocity, immediate send requested).
    fn cart_push_vel(c: &mut Cart, x: f64, z: f64) {
        c.vel[0] += x;
        c.vel[2] += z;
        c.needs_sync = true;
    }

    /// General rule of the push between entities (`this` at `from`, the other at `to`): difference along the
    /// largest axis, × min(1, 1/√d) × 0.05; `None` under 0.01.
    fn entity_push_amount(from: [f64; 3], to: [f64; 3]) -> Option<(f64, f64)> {
        let (xa, za) = (to[0] - from[0], to[2] - from[2]);
        let dd = xa.abs().max(za.abs());
        if dd < f64::from(0.01f32) {
            return None;
        }
        let dd = dd.sqrt();
        let pow = (1.0 / dd).min(1.0);
        let k = pow * f64::from(0.05f32);
        Some((xa / dd * k, za / dd * k))
    }

    /// `mob.push(cart)` (general rule): the mob recoils, the minecart advances if it is empty.
    fn cart_push_mob(&mut self, c: &mut Cart, m: &mut crate::mobs::mob::Mob) {
        let Some((xa, za)) = Self::entity_push_amount(m.pos, c.pos) else { return };
        if m.mount.passengers.is_empty() && self.mob_pushable(m) {
            m.vel[0] -= xa;
            m.vel[2] -= za;
            m.needs_sync = true;
        }
        if !c.is_vehicle() {
            Self::cart_push_vel(c, xa, za);
        }
    }

    /// Can the player be pushed by the minecart (neither passenger nor spectator)?
    fn cart_player_pushable(&self, c: &Cart) -> bool {
        self.game_mode != 3 && !self.life.vit.dead && !c.passengers.contains(&Ent::Player) && !self.player_riding_any()
    }

    /// `player.push(cart)` (general rule). The player's share is only applied to their velocity
    /// on the server side, which the game does not send back to this player (neither "velocity modified" flag nor packet): with no
    /// observable effect, it is not reproduced.
    fn cart_push_player(&mut self, c: &mut Cart) {
        let Some((xa, za)) = Self::entity_push_amount(self.pos, c.pos) else { return };
        if !c.is_vehicle() {
            Self::cart_push_vel(c, xa, za);
        }
    }

    /// The player pushes the minecarts they touch (entity push → `cart.push(player)`): the
    /// minecart moves aside (the player's share, server side only, has no observable effect).
    pub(crate) fn player_pushes_carts(&mut self) {
        if self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        let pb = self.player_aabb();
        let ppos = self.pos;
        let mine = self.pcart.vehicle;
        for c in self.carts.list.iter_mut().flatten() {
            if c.removed || Some(c.id) == mine || !c.bbox().intersects(&pb) {
                continue;
            }
            Self::cart_pushed_by(c, ppos);
        }
    }

    /// Is the minecart itself a passenger? (never here).
    fn cart_is_passenger(&self, _c: &Cart) -> bool {
        false
    }

    /// Movement intention of the player: left/right and
    /// forward/back keys rotated according to their yaw.
    pub(crate) fn player_move_intent(&self) -> [f64; 3] {
        let k = self.mobs.mounts.riding.keys;
        let (fwd, back, left, right) = (k & 1 != 0, k & 2 != 0, k & 4 != 0, k & 8 != 0);
        let l = if left == right { 0.0f32 } else if left { 1.0 } else { -1.0 };
        let f = if fwd == back { 0.0f32 } else if fwd { 1.0 } else { -1.0 };
        let input = [f64::from(l), 0.0, f64::from(f)];
        let len = l2(input);
        if len < 1.0e-7 {
            return [0.0; 3];
        }
        let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
        let r = self.yaw * crate::mobs::mth::DEG;
        let (s, co) = (f64::from(crate::mobs::mth::sin(r)), f64::from(crate::mobs::mth::cos(r)));
        [v[0] * co - v[2] * s, v[1], v[2] * co + v[0] * s]
    }
}
