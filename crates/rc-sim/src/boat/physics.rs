//! The tick of a boat on the integrated server, in the game's order: water status and ticks under
//! water (the passengers are thrown out at 60), shake and damage fading, the base tick of an entity
//! (fluids, fire, below the world), the flotation and the move when no player pilots it (held still
//! otherwise: its pilot's client moves it through `move_vehicle`), the effects of the touched blocks
//! (twice, bubble column included), the bubble column's delay, the paddle strokes, then the push of
//! the touched entities and the boarding of the mobs.

use rc_physics::Aabb;
use rc_physics::mounts::{BoatBody, Status};
use rc_proto_v777::entity::Meta;

use super::riding::player_pilots;
use super::{BUBBLE_TIME, TIME_TO_EJECT};
use crate::minecart::Cart;
use crate::mobs::mob::{Ent, Passenger};
use crate::mobs::recent::vibration::Src;
use crate::server::Server;

/// A paddle turns by π/8 a tick; its stroke sounds as it passes π/4 (once a turn).
const PADDLE_SPEED: f32 = std::f32::consts::PI / 8.0;
const PADDLE_SOUND_ANGLE: f32 = std::f32::consts::PI / 4.0;
const FULL_TURN: f32 = std::f32::consts::PI * 2.0;

impl Cart {
    /// Paddle state of side `side` (0 left, 1 right) as the data say.
    pub(crate) fn paddle_data(&self, side: usize) -> bool {
        matches!(self.meta(11 + side as u8), Some(Meta::Bool(true)))
    }

    /// The paddle data (left, right), sent to the viewers when they change.
    pub(crate) fn set_paddles(&mut self, left: bool, right: bool) {
        self.set_meta(11, Meta::Bool(left));
        self.set_meta(12, Meta::Bool(right));
    }

    pub(crate) fn bubble_time(&self) -> i32 {
        match self.meta(13) {
            Some(Meta::Int(t)) => *t,
            _ => 0,
        }
    }

    fn set_bubble_time(&mut self, t: i32) {
        self.set_meta(13, Meta::Int(t));
    }

    /// A paddle rows only while a player pilots the boat.
    fn paddle_rows(&self, side: usize) -> bool {
        self.paddle_data(side) && player_pilots(self)
    }
}

impl Server {
    /// One tick of the boat `c` (taken out of the list).
    pub(crate) fn boat_tick(&mut self, c: &mut Cart) {
        let b = c.bbox();
        c.boat.float.update_status(&b, &self.world(), &self.phys);
        if c.boat.float.out_of_control >= TIME_TO_EJECT {
            self.cart_eject(c);
        }
        if c.hurt_time > 0 {
            c.hurt_time -= 1;
        }
        if c.damage > 0.0 {
            c.damage -= 1.0;
        }
        c.sync_hurt();
        self.boat_base_tick(c);
        if c.removed {
            return;
        }
        if player_pilots(c) {
            // Its pilot's client is the authority: the server holds it still.
            c.vel = [0.0; 3];
            c.boat.float.rotation = 0.0;
        } else {
            c.set_paddles(false, false);
            self.boat_float(c);
            self.cart_move_without_effects(c, c.vel);
        }
        self.cart_block_effects(c);
        self.cart_block_effects(c);
        if c.removed {
            return;
        }
        self.boat_bubble_column_tick(c);
        self.boat_paddle_strokes(c);
        self.boat_push_and_board(c);
    }

    /// The base tick of an entity, for a boat: fluids (currents, splash, lava), fire (one damage a
    /// second out of lava: a boat is not fire immune), half the fall in lava, below the world.
    fn boat_base_tick(&mut self, c: &mut Cart) {
        self.cart_fluids(c);
        if c.fire > 0 {
            if c.fire % 20 == 0 && !c.in_lava {
                self.cart_hurt(c, "minecraft:on_fire", 1.0, None);
            }
            c.fire -= 1;
        }
        if c.in_lava {
            c.fall_distance *= 0.5;
        }
        if c.pos[1] < f64::from(self.dim.height.min_y - 64) {
            self.cart_discard(c);
        }
        Self::boat_sync_fire_flag(c);
        c.first_tick = false;
    }

    /// The shared "on fire" flag (data 0): burning or kept burning for show (`HasVisualFire`), next
    /// to the glowing flag kept in the boat's fields.
    fn boat_sync_fire_flag(c: &mut Cart) {
        let flag = |key: &str| crate::nbtio::int(&c.nbt, key).is_some_and(|v| v != 0);
        let on_fire = c.fire > 0 || flag("HasVisualFire");
        let flags = i8::from(on_fire) | if flag("Glowing") { 0x40 } else { 0 };
        c.set_meta(0, Meta::Byte(flags));
    }

    /// Buoyancy, gravity and friction of the water status (the land friction is not halved: no
    /// player pilots it here).
    fn boat_float(&self, c: &mut Cart) {
        let mut body = BoatBody { pos: c.pos, vel: c.vel, width: c.width(), height: c.height() };
        let gravity = if c.no_gravity { 0.0 } else { 0.04 };
        c.boat.float.float(&mut body, gravity, false, &self.world(), &self.phys);
        (c.pos, c.vel) = (body.pos, body.vel);
    }

    /// The fall check of a boat's move: its vertical speed kept, its fall forgotten on the
    /// ground and not counted over water; no landing effect, never fall damage.
    pub(crate) fn boat_fall_check(&mut self, c: &mut Cart, moved_y: f64) {
        c.boat.float.remember_vertical_speed(c.vel[1]);
        if c.on_ground {
            c.fall_distance = 0.0;
            return;
        }
        let below = [c.block_pos()[0], c.block_pos()[1] - 1, c.block_pos()[2]];
        // Fluid kind 1: water.
        if moved_y < 0.0 && self.fluid_at(below).kind != 1 {
            c.fall_distance -= f64::from(moved_y as f32);
        }
    }

    // ---------- Bubble columns ----------

    /// Bubble column block `p` touched by the boat. With nothing solid or liquid over the column,
    /// the boat notes it for its delayed effect (and, out of the water, a splash one tick in a
    /// hundred: its sound and particles are the client's own, the server only draws them);
    /// inside the column, it is lifted or pulled like any entity.
    pub(crate) fn boat_in_bubble_column(&mut self, c: &mut Cart, p: [i32; 3]) {
        let pulls_down = self.view().get_named(self.st(p), "drag") == Some("true");
        let above = [p[0], p[1] + 1, p[2]];
        let nothing_above = self.collision_top(above).is_none() && self.fluid_at(above).is_empty();
        if !nothing_above {
            if !player_pilots(c) {
                c.vel[1] = if pulls_down { (c.vel[1] - 0.03).max(-0.3) } else { (c.vel[1] + 0.06).min(0.7) };
            }
            c.fall_distance = 0.0;
            return;
        }
        c.boat.above_bubble_column = true;
        c.boat.bubble_column_pulls_down = pulls_down;
        if c.bubble_time() == 0 {
            c.set_bubble_time(BUBBLE_TIME);
        }
        if !c.boat.float.is_under_water() && c.rng.next_int_bound(100) == 0 {
            // The sound's pitch and the particle's two offsets.
            for _ in 0..3 {
                c.rng.next_float();
            }
            let by = player_pilots(c).then_some(Src::Living(Ent::Player));
            self.game_event_src("minecraft:splash", c.pos, by, None);
        }
    }

    /// The delayed effect of a bubble column under the boat: after 60 ticks over it, a column that
    /// pulls down throws the passengers out and drags the boat down (event 71), one that pushes up
    /// throws it up (event 72). Leaving the column resets the delay.
    fn boat_bubble_column_tick(&mut self, c: &mut Cart) {
        if !c.boat.above_bubble_column {
            c.set_bubble_time(0);
        }
        let mut time = c.bubble_time();
        if time <= 0 {
            return;
        }
        time -= 1;
        c.set_bubble_time(time);
        if BUBBLE_TIME - time - 1 > 0 && time == 0 {
            let down = c.boat.bubble_column_pulls_down;
            if down {
                self.cart_eject(c);
            }
            self.cart_event(c, if down { 71 } else { 72 });
            Self::boat_bubble_column_effect(c, down);
        }
        c.boat.above_bubble_column = false;
    }

    /// The bubble column's push on a boat the server moves (a piloted one gets it from its
    /// client, on the entity event).
    fn boat_bubble_column_effect(c: &mut Cart, down: bool) {
        if player_pilots(c) {
            return;
        }
        if down {
            c.vel[1] -= 0.7;
        } else {
            c.vel[1] = 0.6;
        }
    }

    // ---------- Pushes and boarding ----------

    /// The entities touching the boat's box (0.2 wider aside, 0.01 thinner): a mob boards it when
    /// no player pilots it, a seat is free, the mob rides nothing, is narrower than the boat and
    /// may be pushed onto boats; any other touched entity is pushed by the boat.
    fn boat_push_and_board(&mut self, c: &mut Cart) {
        let area = boat_push_area(c);
        let boards = !player_pilots(c);
        let own = Some(c.uuid);
        // Mobs touched, in list order (the boat's own passengers share its vehicle: no push).
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .filter(|m| m.alive() && m.touches(&area) && !c.passengers.contains(&Passenger::Mob(m.id)))
            .filter(|m| self.mob_pushable(m) && self.teams_allow_push(own, Some(m.uuid)))
            .map(|m| m.id)
            .collect();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let Some(mut m) = self.mobs.list.take(i) else { continue };
            if boards && self.boat_may_take(c, &m) {
                self.cart_mob_start_riding(c, &mut m);
            } else {
                self.boat_push_mob(c, &mut m);
            }
            self.mobs.list.put(i, m);
        }
        // The player is never boarded (players may not be pushed onto boats): pushed only, its
        // own share having no effect the server sends back.
        let player_touched = self.game_mode != 3
            && !self.life.vit.dead
            && !c.passengers.contains(&self.player_passenger())
            && self.player_aabb().intersects(&area)
            && self.teams_allow_push(own, None);
        if player_touched && self.pos[1] <= c.pos[1] {
            boat_moves_aside(c, self.pos);
        }
        // Other boats and minecarts touched.
        for i in 0..self.carts.list.len() {
            let Some(mut o) = self.carts.list[i].take() else { continue };
            if !o.removed && o.bbox().intersects(&area) && self.teams_allow_push(own, Some(o.uuid)) {
                boat_push_vehicle(c, &mut o);
            }
            self.carts.list[i] = Some(o);
        }
    }

    /// The boarding test of a touched mob, before its own riding test.
    fn boat_may_take(&self, c: &Cart, m: &crate::mobs::mob::Mob) -> bool {
        let crate::minecart::CKind::Boat(t) = c.kind else { return false };
        c.passengers.len() + c.foreign.len() < t.max_passengers()
            && m.mount.vehicle.is_none()
            && f64::from(m.width()) < c.width()
            && !self.bz_etag(m.kind, "minecraft:cannot_be_pushed_onto_boats")
    }

    /// The boat pushes mob `m` when the mob's feet are not above the boat's bottom: the general
    /// rule between two entities (the boat moves aside when empty, the mob when it carries nobody).
    pub(crate) fn boat_push_mob(&self, c: &mut Cart, m: &mut crate::mobs::mob::Mob) {
        if m.pos()[1] > c.pos[1] || crate::mobs::illagers::no_physics(m) {
            return;
        }
        let Some((xa, za)) = Self::entity_push_amount(c.pos, m.pos()) else { return };
        if !c.is_vehicle() {
            c.vel[0] -= xa;
            c.vel[2] -= za;
            c.needs_sync = true;
        }
        if m.mount.passengers.is_empty() && self.mob_pushable(m) {
            m.vel[0] += xa;
            m.vel[2] += za;
            m.needs_sync = true;
        }
    }

    // ---------- Paddles ----------

    /// Each rowing paddle turns; its stroke sounds (water or land) at the boat's side as it
    /// passes the water. A paddle at rest goes back to 0.
    fn boat_paddle_strokes(&mut self, c: &mut Cart) {
        for side in 0..2 {
            if !c.paddle_rows(side) {
                c.boat.paddle_angles[side] = 0.0;
                continue;
            }
            let angle = c.boat.paddle_angles[side];
            let passes_water =
                angle % FULL_TURN <= PADDLE_SOUND_ANGLE && (angle + PADDLE_SPEED) % FULL_TURN >= PADDLE_SOUND_ANGLE;
            if !c.silent && passes_water {
                self.boat_paddle_sound(c, side);
            }
            c.boat.paddle_angles[side] = angle + PADDLE_SPEED;
        }
    }

    /// The stroke of paddle `side`, beside the boat: in water or on land (none in the air).
    fn boat_paddle_sound(&mut self, c: &mut Cart, side: usize) {
        // The status is read again here, after the move.
        let status = c.boat.float.status_now(&c.bbox(), &self.world(), &self.phys);
        let sound = match status {
            Status::InWater | Status::UnderWater | Status::UnderFlowingWater => "minecraft:entity.boat.paddle_water",
            Status::OnLand => "minecraft:entity.boat.paddle_land",
            Status::InAir => return,
        };
        let view = rc_physics::mounts::view_vector(c.yaw, c.pitch);
        let (dx, dz) = if side == 1 { (-view[2], view[0]) } else { (view[2], -view[0]) };
        let pitch = 0.8 + 0.4 * c.rng.next_float();
        self.world_sound(sound, 6, [c.pos[0] + dx, c.pos[1], c.pos[2] + dz], 1.0, pitch);
    }
}

/// The box a boat pushes in: its own, 0.2 wider on each side and 0.01 thinner at both ends.
fn boat_push_area(c: &Cart) -> Aabb {
    let b = c.bbox();
    let (side, thin) = (f64::from(0.2f32), f64::from(0.01f32));
    Aabb {
        min: [b.min[0] - side, b.min[1] + thin, b.min[2] - side],
        max: [b.max[0] + side, b.max[1] - thin, b.max[2] + side],
    }
}

/// An empty boat moves aside from an entity at `other` that pushes it (or that it pushes).
pub(crate) fn boat_moves_aside(c: &mut Cart, other: [f64; 3]) {
    if c.is_vehicle() {
        return;
    }
    if let Some((xa, za)) = Server::entity_push_amount(c.pos, other) {
        c.vel[0] -= xa;
        c.vel[2] -= za;
        c.needs_sync = true;
    }
}

/// Boat `c` pushes the touched vehicle `o`: another boat whose bottom is below its top, any
/// other vehicle whose bottom is not above its own; then each moves aside unless it carries someone.
pub(crate) fn boat_push_vehicle(c: &mut Cart, o: &mut Cart) {
    let reaches = if o.kind.is_boat() { o.pos[1] < c.bbox().max[1] } else { o.pos[1] <= c.pos[1] };
    if !reaches {
        return;
    }
    let Some((xa, za)) = Server::entity_push_amount(c.pos, o.pos) else { return };
    if !c.is_vehicle() {
        c.vel[0] -= xa;
        c.vel[2] -= za;
        c.needs_sync = true;
    }
    if !o.is_vehicle() {
        o.vel[0] += xa;
        o.vel[2] += za;
        o.needs_sync = true;
    }
}
