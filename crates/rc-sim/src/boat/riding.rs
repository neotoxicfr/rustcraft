//! Passengers and pilot of a boat: the seats (two, one beside a chest), who may board, where a
//! passenger gets off, the right click, and the player's piloting (`move_vehicle`, `paddle_boat`).

use super::BoatType;
use crate::minecart::{CKind, Cart};
use crate::mobs::mob::{Ent, Mob, Passenger};
use crate::mobs::mth;
use crate::server::Server;

/// A player pilots the boat: its first passenger is a player (a mob never steers a boat).
pub(crate) fn player_pilots(c: &Cart) -> bool {
    c.kind.is_boat() && c.passengers.first().is_some_and(|p| p.is_player())
}

/// Seat (feet) of passenger `who` of boat `c` (`rider`: its mob, `None` for a player): the boat's
/// seat point, ahead or behind when two ride it (an animal a little further ahead), turned by the
/// boat's yaw, less the passenger's own attachment height.
pub(crate) fn boat_seat(c: &Cart, t: BoatType, who: Passenger, rider: Option<&Mob>) -> [f64; 3] {
    let rank = c.passengers.iter().position(|p| *p == who).unwrap_or(c.passengers.len());
    let mut offset = t.single_passenger_offset();
    if c.passengers.len() > 1 {
        offset = if rank == 0 { 0.2 } else { -0.6 };
        if rider.is_some_and(|m| m.kind.is_animal()) {
            offset += 0.2;
        }
    }
    let angle = -c.yaw * mth::DEG;
    let (sin, cos) = (f64::from(mth::sin(angle)), f64::from(mth::cos(angle)));
    let attach = crate::mobs::mounts::riding::vehicle_attach(rider);
    let offset = f64::from(offset);
    [c.pos[0] + offset * sin, c.pos[1] + t.ride_height() - attach, c.pos[2] + offset * cos]
}

impl Server {
    /// Can boat `c` take one more passenger: a free seat, and its eyes out of the water.
    pub(crate) fn boat_can_add_passenger(&self, c: &Cart) -> bool {
        let CKind::Boat(t) = c.kind else { return false };
        c.passengers.len() + c.foreign.len() < t.max_passengers() && !self.boat_eyes_in_water(c)
    }

    /// The boat's eyes (at its top, tested 1/9 lower) are in water.
    fn boat_eyes_in_water(&self, c: &Cart) -> bool {
        let eye_y = c.pos[1] + c.kind.eye_height() - f64::from(0.111_111_11f32);
        let p = [c.pos[0].floor() as i32, eye_y.floor() as i32, c.pos[2].floor() as i32];
        self.fluid_at(p).kind == 1 && f64::from(p[1]) + self.fluid_height_at(p) > eye_y
    }

    /// Right click on a boat: boarding (not while sneaking, nor once it has spent 60 ticks under
    /// water); a chest boat opens its chest when sneaking or when no seat is left.
    pub(crate) fn boat_use(&mut self, c: &mut Cart) {
        let out_of_control = c.boat.float.out_of_control >= super::TIME_TO_EJECT;
        if !self.sneaking && !out_of_control && self.player_start_riding_cart(c) {
            return;
        }
        if !c.kind.has_chest() || (self.boat_can_add_passenger(c) && !self.sneaking) {
            return;
        }
        if self.cart_open_menu(c) {
            self.game_event_src(
                "minecraft:container_open",
                c.pos,
                Some(crate::mobs::recent::vibration::Src::Living(Ent::Player)),
                None,
            );
            self.anger_nearby_piglins(true);
        }
    }

    /// Where a passenger of width `width` looking along `yaw` gets off boat `c`: beside the boat
    /// in that direction, on the floor of the block at the boat's top or of the one below (never
    /// over water), the first of its poses `(width, height)` with room; otherwise on top of the
    /// boat.
    pub(crate) fn boat_dismount_location(&self, c: &Cart, width: f32, yaw: f32, poses: &[(f32, f32)]) -> [f64; 3] {
        let top = c.bbox().max[1];
        let reach = c.width() as f32 * std::f32::consts::SQRT_2;
        let dir = crate::mobs::mounts::riding::escape_vector(f64::from(reach), f64::from(width), yaw);
        let (x, z) = (c.pos[0] + dir[0], c.pos[2] + dir[2]);
        let block = [x.floor() as i32, top.floor() as i32, z.floor() as i32];
        let below = [block[0], block[1] - 1, block[2]];
        if self.fluid_at(below).kind != 1 {
            let floors = [block, below].map(|p| {
                let floor = self.block_floor_height(p);
                (floor.is_finite() && floor < 1.0).then(|| [x, f64::from(p[1]) + floor, z])
            });
            for &(w, h) in poses {
                for target in floors.iter().flatten() {
                    let b = Mob::bbox_at(*target, w, h);
                    if rc_physics::is_free(&self.world(), &b) && self.box_in_border(&b) {
                        return *target;
                    }
                }
            }
        }
        [c.pos[0], top, c.pos[2]]
    }

    /// A mob passenger seated: its body turned like the boat (an animal sideways when the boat is
    /// full), its head kept within its reach of the body; a breeze turns freely.
    pub(crate) fn boat_seat_mob(&self, c: &Cart, m: &mut Mob) {
        let CKind::Boat(t) = c.kind else { return };
        m.set_pos(boat_seat(c, t, Passenger::Mob(m.id), Some(m)));
        if self.bz_etag(m.kind, "minecraft:can_turn_in_boats") {
            return;
        }
        // The boat's own turn would be added here: the server never steers a boat, so it is 0.
        let sideways = m.kind.is_animal() && c.passengers.len() == t.max_passengers();
        let body = if sideways { c.yaw + if m.id % 2 == 0 { 90.0 } else { 270.0 } } else { c.yaw };
        m.body_yaw = body;
        m.yaw = body;
        let limit = crate::mobs::goals::max_head_y(m.kind);
        let delta = mth::wrap(body - m.head_yaw);
        m.head_yaw += delta - delta.clamp(-limit, limit);
    }

    // ---------- The player's piloting ----------

    /// The installed player pilots boat `c`: it is its first passenger (the second player of a
    /// shared boat neither paddles nor moves it).
    fn installed_player_pilots(&self, c: &Cart) -> bool {
        player_pilots(c) && c.passengers.first() == Some(&self.player_passenger())
    }

    /// `paddle_boat`: the paddles of the boat the player pilots.
    pub(crate) fn boat_paddles_from_pilot(&mut self, left: bool, right: bool) {
        let Some(id) = self.pcart.vehicle else { return };
        let pilots = self.cart_ref(id).is_some_and(|c| self.installed_player_pilots(c));
        if pilots && let Some(c) = self.cart_mut(id) {
            c.set_paddles(left, right);
        }
    }

    /// `move_vehicle` from the player piloting a boat: the boat goes where the client put it,
    /// with the refusals of any vehicle (the horizontal difference left after the move with
    /// collisions too large in free room, or a new obstacle at the target), sent back otherwise.
    pub(crate) fn boat_moved_by_pilot(&mut self, to: [f64; 3], yaw: f32, pitch: f32, on_ground: bool) {
        let Some(id) = self.pcart.vehicle else { return };
        let Some((i, mut c)) = self.cart_take(id) else { return };
        if self.installed_player_pilots(&c) {
            self.boat_follow_pilot(&mut c, to, [yaw, pitch], on_ground);
        }
        self.cart_put(i, c);
    }

    fn boat_follow_pilot(&mut self, c: &mut Cart, to: [f64; 3], rot: [f32; 2], on_ground: bool) {
        let to = [to[0].clamp(-3.0e7, 3.0e7), to[1].clamp(-2.0e7, 2.0e7), to[2].clamp(-3.0e7, 3.0e7)];
        let (old, old_box) = (c.pos, c.bbox());
        let good = self.mobs.mounts.riding.last_good;
        self.cart_move_without_effects(c, [to[0] - good[0], to[1] - good[1], to[2] - good[2]]);
        // The game tests "> −0.5 or < 0.5": always true, the vertical difference is ignored.
        let (ex, ez) = (to[0] - c.pos[0], to[2] - c.pos[2]);
        let fail = ex * ex + ez * ez > 0.0625;
        let room = self.world_for_room(Self::room_for_object(c.id, c.pos));
        let at_target = c.bbox().offset(to[0] - c.pos[0], to[1] - c.pos[1], to[2] - c.pos[2]);
        let refused = (fail && rc_physics::is_free(&room, &old_box))
            || crate::play::move_meets_new_obstacle(&room, &old_box, &at_target, &mut Vec::new());
        (c.yaw, c.pitch) = (mth::wrap(rot[0]), mth::wrap(rot[1]));
        if refused {
            c.pos = old;
            let _ = self.send(rc_client::Event::MoveVehicle { pos: c.pos, yaw: c.yaw, pitch: c.pitch });
            return;
        }
        c.pos = to;
        c.on_ground = on_ground;
        self.mobs.mounts.riding.last_good = c.pos;
    }
}
