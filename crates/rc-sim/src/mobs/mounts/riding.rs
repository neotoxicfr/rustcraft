//! Passengers and vehicles (rider placement): mobs
//! ridden by mobs (zombie horse and its zombie, camel husk and its two
//! riders, trap horses), mounted player; tick of passengers by their vehicle, seat,
//! dismounting, packets of the mounted player (`move_vehicle`, `player_input`, mount jump,
//! inventory), navigation lent by the mount to the rider who steers it.

use rc_client::{EntityEvent, Event};
use rc_proto::Reader;
use rc_proto::varint::read_varint;

use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, cos, sin};
use super::super::species::Kind;
use super::{dismounts_underwater, handles, is_camel, saddled};
use crate::server::Server;

/// Mounted player: direct vehicle, delay before remounting, keys held, last safe position of the
/// piloted vehicle.
#[derive(Debug, Clone, Default)]
pub struct PlayerRide {
    pub vehicle: Option<i32>,
    pub boarding_cooldown: i32,
    pub keys: u8,
    /// Closing in progress: the player's vehicle is not written into the chunks.
    pub unloading: bool,
    pub last_good: [f64; 3],
}

/// "Vehicle" attachment of a passenger (declared, ×0.5 for a baby).
pub(crate) fn vehicle_attach(rider: Option<&Mob>) -> f64 {
    let Some(r) = rider else { return 0.6 };
    // F13: piglins (babies: own attachment).
    if let Some(v) = super::super::piglins::vehicle_attach(r) {
        return v;
    }
    let off = match r.kind {
        Kind::Zombie
        | Kind::Husk
        | Kind::ZombieVillager
        | Kind::Skeleton
        | Kind::Stray
        | Kind::Bogged
        | Kind::Parched => -0.7,
        Kind::Giant => -3.75,
        Kind::Phantom => -0.125,
        // F07: illagers (ravager riders), vex.
        Kind::Pillager | Kind::Vindicator | Kind::Evoker | Kind::Illusioner => -0.6,
        Kind::Vex => 0.04,
        // F12: wither skeleton.
        Kind::WitherSkeleton => -0.875,
        _ => 0.0,
    };
    -off * if r.is_baby() { 0.5 } else { 1.0 }
}

/// Declared "passenger" attachment point (before rotation), current dimensions.
fn declared_point(v: &Mob) -> [f64; 3] {
    let baby = v.is_baby();
    match v.kind {
        Kind::Horse => [0.0, if baby { f64::from((1.6f32 - 0.125) * 0.7) } else { 1.443_75 }, 0.0],
        Kind::SkeletonHorse | Kind::ZombieHorse => {
            [0.0, if baby { f64::from((1.6f32 - 0.25) * 0.7) } else { 1.318_75 }, 0.0]
        }
        Kind::Donkey | Kind::Mule => {
            let h = if v.kind == Kind::Donkey { 1.5f32 } else { 1.6 };
            if baby {
                [0.0, f64::from((h + 0.031_25) * 0.5), f64::from(-0.3125f32 * 0.5)]
            } else {
                [0.0, if v.kind == Kind::Donkey { 1.1125 } else { 1.2125 }, 0.0]
            }
        }
        Kind::Llama | Kind::TraderLlama => {
            let s = if baby { 0.5 } else { 1.0 };
            [0.0, 1.37 * s, -0.3 * s]
        }
        // F07: ravager (raid rider).
        Kind::Ravager => [0.0, 2.2625, -0.0625],
        // F09: nautiluses (baby: 0.5).
        Kind::Nautilus | Kind::ZombieNautilus => [0.0, if baby { 0.5 } else { f64::from(1.1375f32) }, 0.0],
        // F12: baby strider (own attachment 0.65625; the adult keeps its height).
        Kind::Strider if baby => [0.0, 0.656_25, 0.0],
        // F13: hoglin, zoglin, piglins.
        k if super::super::piglins::handles(k) && super::super::piglins::passenger_point(v).is_some() => {
            super::super::piglins::passenger_point(v).unwrap_or_default()
        }
        // W8c: declared attachments of the pig and the wolf (baby: half).
        Kind::Pig => [0.0, 0.868_75 * if baby { 0.5 } else { 1.0 }, 0.0],
        Kind::Wolf => {
            let s = if baby { 0.5 } else { 1.0 };
            [0.0, 0.818_75 * s, -0.0625 * s]
        }
        _ => [0.0, f64::from(v.height()), 0.0],
    }
}

/// Average of the declared "passenger" attachment points, without rotation (where sheared
/// equipment falls).
pub(crate) fn passenger_average(v: &Mob) -> [f64; 3] {
    if v.kind == Kind::HappyGhast {
        let pts: Vec<[f64; 3]> =
            (0..4).filter_map(|r| super::super::recent::happy_ghast::passenger_point(v, r)).collect();
        let n = pts.len().max(1) as f64;
        return std::array::from_fn(|i| pts.iter().map(|p| p[i]).sum::<f64>() / n);
    }
    declared_point(v)
}

/// Rotation by −yaw around Y.
fn y_rot(p: [f64; 3], yaw: f32) -> [f64; 3] {
    let a = -yaw * DEG;
    let (s, c) = (f64::from(sin(a)), f64::from(cos(a)));
    [p[0] * c + p[2] * s, p[1], p[2] * c - p[0] * s]
}

/// Seat height of the camel according to the pose.
pub fn camel_anchor(v: &Mob, front: bool, game_time: i64, height: f32, scale: f32) -> f64 {
    let age_off = if v.is_baby() { 0.093_75 } else { 0.375 };
    let mut base = f64::from(height) - age_off;
    let d = scale * 1.43;
    let drop = d - scale * 0.2;
    let bottom = d - drop;
    let sitting = v.mount.last_pose_tick < 0;
    let time = game_time - v.mount.last_pose_tick.abs();
    let transition = time < if sitting { 40 } else { 52 };
    if transition {
        let duration = if sitting { 40.0 } else { 52.0 };
        let (half, flex) = if sitting {
            (28.0, if front { 0.5 } else { 0.1 })
        } else {
            (if front { 24.0 } else { 32.0 }, if front { 0.6 } else { 0.35 })
        };
        let t = (time as f32).clamp(0.0, duration);
        let first = t < half;
        let part = if first { t / half } else { (t - half) / (duration - half) };
        let flex_pt = d - flex * drop;
        let lerp = |a: f32, b: f32| a + part * (b - a);
        base += f64::from(if sitting {
            if first { lerp(d, flex_pt) } else { lerp(flex_pt, bottom) }
        } else if first {
            lerp(bottom - d, bottom - flex_pt)
        } else {
            lerp(bottom - flex_pt, 0.0)
        });
    }
    if sitting && !transition {
        base += f64::from(bottom);
    }
    base
}

impl Server {
    /// Attachment point of the passenger of rank `rank`, rotated.
    pub(crate) fn passenger_point(&self, v: &Mob, rank: usize, rider_animal: bool) -> [f64; 3] {
        // F14: happy ghast, four seats.
        if let Some(p) = super::super::recent::happy_ghast::passenger_point(v, rank) {
            return y_rot(p, v.yaw);
        }
        if is_camel(v.kind) {
            let scale = super::age_scale(v);
            let count = v.mount.passengers.len();
            let front = rank == 0;
            let h = v.height();
            let y = if v.removed { 0.01 } else { camel_anchor(v, front, self.time, h, scale) };
            let mut z = 0.5f32;
            if count > 1 {
                if !front {
                    z = -0.7;
                }
                if rider_animal {
                    z += 0.2;
                }
            }
            return y_rot([0.0, y, f64::from(z * scale)], v.yaw);
        }
        let mut p = declared_point(v);
        if handles(v.kind) && !super::is_llama(v.kind) {
            // Rearing: old value of the animation, age scale.
            let a = f64::from(v.mount.stand_anim_o * super::age_scale(v));
            p[1] += 0.15 * a;
            p[2] += -0.7 * a;
        }
        y_rot(p, v.yaw)
    }

    /// Seat of a passenger (feet): vehicle position + point − passenger attachment.
    pub(crate) fn seat_of(&self, v: &Mob, rank: usize, rider: Option<&Mob>) -> [f64; 3] {
        let animal = rider.is_some_and(|r| r.kind.is_animal());
        let p = self.passenger_point(v, rank, animal);
        let a = vehicle_attach(rider);
        [v.pos[0] + p[0], v.pos[1] + p[1] - a, v.pos[2] + p[2]]
    }

    /// Does the player pilot this vehicle (saddled, player first)?
    pub(crate) fn player_controls(v: &Mob) -> bool {
        ((handles(v.kind) || super::super::sealife::is_nautilus(v.kind))
            && saddled(v)
            && v.mount.passengers.first() == Some(&Ent::Player))
            // F12: saddled strider, player holding the warped fungus on a stick.
            || super::super::nether::strider::player_controls(v)
            // F14: harnessed happy ghast, outside immobility.
            || super::super::recent::happy_ghast::player_controls(v)
    }

    /// Mob that steers this vehicle (→ mob), without AI cut;
    /// a passenger of the `non_controlling_rider` tag does not steer.
    pub(crate) fn mob_controller(v: &Mob) -> Option<i32> {
        if Self::player_controls(v) || v.no_ai {
            return None;
        }
        match v.mount.passengers.first() {
            Some(Ent::Mob(id)) if !v.mount.passive_riders.contains(id) => Some(*id),
            _ => None,
        }
    }

    /// Cut goal flags: movement, jump, look if a
    /// mob steers.
    pub(crate) fn control_flags(v: &Mob) -> u8 {
        use super::super::goals::{JUMP, LOOK, MOVE};
        if Self::mob_controller(v).is_some() { MOVE | JUMP | LOOK } else { 0 }
    }

    // ---------- Mounting ----------

    /// The player mounts the mount: free space, player first, seat, teleportation.
    pub(crate) fn player_start_riding(&mut self, m: &mut Mob) -> bool {
        if self.mobs.mounts.riding.vehicle == Some(m.id)
            || self.sneaking
            || self.mobs.mounts.riding.boarding_cooldown > 0
        {
            return false;
        }
        let room = if is_camel(m.kind) {
            m.mount.passengers.len() <= 2
        } else if let Some(n) = super::super::recent::happy_ghast::max_passengers(m) {
            m.mount.passengers.len() < n
        } else {
            m.mount.passengers.is_empty()
        };
        if !room {
            return false;
        }
        if self.mobs.mounts.riding.vehicle.is_some() {
            self.player_stop_riding();
        }
        self.cushion_stand_up(); // sitting on a cushion: leaves it first
        self.mobs.mounts.riding.vehicle = Some(m.id);
        self.prog.ride_started = true; // W8c: `started_riding`
        // The player goes in front if the first passenger is not a player.
        if m.mount.passengers.first().is_some_and(|p| *p != Ent::Player) {
            m.mount.passengers.insert(0, Ent::Player);
        } else {
            m.mount.passengers.push(Ent::Player);
        }
        // Adding an equine passenger: the player takes the animal's view rotation.
        self.yaw = m.yaw;
        self.pitch = m.pitch;
        // `entity_mount` event to the mount, emitted by the player.
        self.game_event_at("minecraft:entity_mount", m.pos, Some(Ent::Player));
        let rank = m.mount.passengers.iter().position(|p| *p == Ent::Player).unwrap_or(0);
        let seat = self.seat_of(m, rank, None);
        self.mobs.mounts.riding.last_good = m.pos;
        self.teleport_to(seat);
        self.last_good = seat;
        self.send_passengers(m);
        true
    }

    /// Mob that mounts another (forced or not); true if mounted.
    pub(crate) fn mob_start_riding(&mut self, rider: &mut Mob, v: &mut Mob, force: bool) -> bool {
        if rider.mount.vehicle == Some(v.id) {
            return false;
        }
        if !force {
            let room = if is_camel(v.kind) {
                v.mount.passengers.len() <= 2
            } else if let Some(n) = super::super::recent::happy_ghast::max_passengers(v) {
                v.mount.passengers.len() < n
            } else {
                v.mount.passengers.is_empty()
            };
            if rider.mount.boarding_cooldown > 0 || !room {
                return false;
            }
        }
        rider.mount.vehicle = Some(v.id);
        v.mount.passengers.push(Ent::Mob(rider.id));
        if rider.kind.non_controlling_rider() {
            v.mount.passive_riders.push(rider.id);
        }
        let rank = v.mount.passengers.len() - 1;
        rider.pos = self.seat_of(v, rank, Some(rider));
        rider.old = rider.pos;
        if handles(v.kind) && !super::is_llama(v.kind) {
            rider.yaw = v.yaw;
            rider.pitch = v.pitch;
        }
        // W8c: the player carried (even indirectly) by the vehicle: `started_riding`.
        let mut cur = self.mobs.mounts.riding.vehicle;
        for _ in 0..16 {
            let Some(id) = cur else { break };
            if id == v.id {
                self.prog.ride_started = true;
                break;
            }
            cur = if id == rider.id {
                rider.mount.vehicle
            } else {
                self.mobs.mob_ref_any(id).and_then(|m| m.mount.vehicle)
            };
        }
        true
    }

    // ---------- Dismounting ----------

    /// Dismount location (of the equine): side of the leading hand
    /// first, then the other, otherwise the animal's position.
    pub(crate) fn dismount_location(&self, v: &Mob, width: f32, poses: &[(f32, f32)]) -> [f64; 3] {
        // F12: the strider looks for a lava-free floor around the passenger's gaze.
        if v.kind == Kind::Strider {
            return super::super::nether::strider::dismount_location(self, v, width, poses, self.yaw);
        }
        // F14: happy ghast, top of the box.
        if let Some(p) = super::super::recent::happy_ghast::dismount_location(v) {
            return p;
        }
        for side in [90.0f32, -90.0] {
            let dir = escape_vector(f64::from(v.width()), f64::from(width), v.yaw + side);
            if let Some(p) = self.dismount_in_direction(v, dir, width, poses) {
                return p;
            }
        }
        v.pos
    }

    fn dismount_in_direction(&self, v: &Mob, dir: [f64; 3], width: f32, poses: &[(f32, f32)]) -> Option<[f64; 3]> {
        let tx = v.pos[0] + dir[0];
        let ty = v.bbox().min[1];
        let tz = v.pos[2] + dir[2];
        let limit = v.bbox().max[1] + 0.75;
        for &(w, h) in poses {
            let mut by = ty.floor() as i32;
            let (bx, bz) = (tx.floor() as i32, tz.floor() as i32);
            loop {
                let floor = self.block_floor_height([bx, by, bz]);
                if f64::from(by) + floor > limit {
                    break;
                }
                if floor.is_finite() && floor < 1.0 {
                    let loc = [tx, f64::from(by) + floor, tz];
                    let b = Mob::bbox_at(loc, w.max(width), h);
                    // Free space and within the world border.
                    if rc_physics::is_free(&self.world(), &b) && self.box_in_border(&b) {
                        return Some(loc);
                    }
                }
                by += 1;
                if f64::from(by) >= limit {
                    break;
                }
            }
        }
        None
    }

    /// Top of the shape, otherwise that of the one below − 1 (−∞ if low).
    fn block_floor_height(&self, p: [i32; 3]) -> f64 {
        if let Some(t) = self.collision_top(p) {
            return t;
        }
        match self.collision_top([p[0], p[1] - 1, p[2]]) {
            Some(b) if b >= 1.0 => b - 1.0,
            _ => f64::NEG_INFINITY,
        }
    }

    /// The player dismounts: teleported to the dismount location.
    pub(crate) fn player_stop_riding(&mut self) {
        let Some(target) = self.player_unmount() else { return };
        self.teleport_to(target);
        self.last_good = target;
    }

    /// The player leaves any mount, mob, cushion or minecart, without being repositioned
    /// (the vehicle's dismount): teleportation, respawn, dimension change; the following position
    /// is set by the caller.
    pub(crate) fn player_remove_vehicle(&mut self) {
        self.player_unmount();
        self.cushion_unseat();
        self.cart_remove_player(); // G5-F06
    }

    /// Exit from the mob vehicle; returns the dismount location.
    fn player_unmount(&mut self) -> Option<[f64; 3]> {
        let vid = self.mobs.mounts.riding.vehicle.take()?;
        self.mobs.mounts.riding.boarding_cooldown = 0; // A forced dismount of the player resets the delay to 0
        let poses = [(0.6, 1.8), (0.6, 1.5), (0.6, 0.6)];
        let target = match self.mob_index(vid) {
            Some(i) => {
                let mut v = self.mobs.list[i].take().expect("vehicle");
                v.mount.passengers.retain(|p| *p != Ent::Player);
                super::super::recent::happy_ghast::on_passenger_removed(self, &mut v);
                let t = if v.removed { self.pos } else { self.dismount_location(&v, 0.6, &poses) };
                // `entity_dismount` event to the former mount, emitted by the player.
                self.game_event_at("minecraft:entity_dismount", v.pos, Some(Ent::Player));
                self.mobs.list[i] = Some(v);
                t
            }
            None => self.pos,
        };
        Some(target)
    }

    /// Variant when the vehicle is already out of the list.
    pub(crate) fn player_dismount_from(&mut self, v: &mut Mob) {
        if self.mobs.mounts.riding.vehicle != Some(v.id) {
            return;
        }
        self.mobs.mounts.riding.vehicle = None;
        self.mobs.mounts.riding.boarding_cooldown = 0; // A forced dismount of the player resets the delay to 0
        v.mount.passengers.retain(|p| *p != Ent::Player);
        super::super::recent::happy_ghast::on_passenger_removed(self, v);
        let poses = [(0.6, 1.8), (0.6, 1.5), (0.6, 0.6)];
        let target = if v.removed { self.pos } else { self.dismount_location(v, 0.6, &poses) };
        self.game_event_at("minecraft:entity_dismount", v.pos, Some(Ent::Player));
        self.teleport_to(target);
        self.last_good = target;
    }

    /// A mob dismounts from its vehicle (out of the list).
    pub(crate) fn mob_dismount_from(&mut self, rider: &mut Mob, v: &mut Mob) {
        v.mount.passengers.retain(|p| *p != Ent::Mob(rider.id));
        v.mount.passive_riders.retain(|i| *i != rider.id);
        super::super::recent::happy_ghast::on_passenger_removed(self, v);
        rider.mount.vehicle = None;
        rider.mount.boarding_cooldown = 60;
        let poses = [(rider.width(), rider.height())];
        let t = if v.removed || rider.removed { rider.pos } else { self.dismount_location(v, rider.width(), &poses) };
        rider.pos = t;
        rider.old = t;
        rider.needs_sync = true;
    }

    /// All dismount.
    pub(crate) fn eject_passengers(&mut self, v: &mut Mob) {
        for p in v.mount.passengers.clone() {
            match p {
                Ent::Player => self.player_dismount_from(v),
                Ent::Mob(id) => {
                    if let Some(i) = self.mob_index(id)
                        && let Some(mut r) = self.mobs.list[i].take()
                    {
                        self.mob_dismount_from(&mut r, v);
                        self.mobs.list[i] = Some(r);
                    } else {
                        v.mount.passengers.retain(|q| *q != p);
                    }
                }
            }
        }
    }

    // ---------- Tick ----------

    /// Passenger of a present vehicle: ticked by its vehicle (true), otherwise it dismounts.
    pub(crate) fn ticked_by_vehicle(&mut self, m: &mut Mob) -> bool {
        let Some(vid) = m.mount.vehicle else { return false };
        let ok = self.mob_ref(vid).is_some_and(|v| !v.removed && v.mount.passengers.contains(&Ent::Mob(m.id)));
        if ok {
            return true;
        }
        // G5-F06: passenger of a minecart, ticked by it.
        if self.cart_ref(vid).is_some_and(|c| c.passengers.contains(&Ent::Mob(m.id))) {
            return true;
        }
        if let Some(i) = self.mob_index(vid)
            && let Some(mut v) = self.mobs.list[i].take()
        {
            self.mob_dismount_from(m, &mut v);
            self.mobs.list[i] = Some(v);
        } else {
            m.mount.vehicle = None;
        }
        false
    }

    /// Passengers of vehicle `vid` ticked after it (the passenger's tick → the mounted one's tick),
    /// in list order, then their own passengers.
    pub(crate) fn tick_passengers(&mut self, vid: i32) {
        let Some(list) = self.mob_ref(vid).map(|v| v.mount.passengers.clone()) else { return };
        for p in list {
            match p {
                Ent::Player => self.player_ride_tick(vid),
                Ent::Mob(pid) => {
                    let Some(i) = self.mob_index(pid) else { continue };
                    let Some(mut r) = self.mobs.list[i].take() else { continue };
                    let alive_vehicle = self.mob_ref(vid).is_some_and(|v| !v.removed);
                    if r.removed || r.mount.vehicle != Some(vid) || !alive_vehicle {
                        if let Some(vi) = self.mob_index(vid)
                            && let Some(mut v) = self.mobs.list[vi].take()
                        {
                            self.mob_dismount_from(&mut r, &mut v);
                            self.mobs.list[vi] = Some(v);
                        }
                        self.mobs.list[i] = Some(r);
                        continue;
                    }
                    // Zero speed, full tick, then seat.
                    r.vel = [0.0; 3];
                    self.tick_mob(&mut r);
                    // Dismounted during its tick (eyes under water): stays at the dismount location.
                    if r.mount.vehicle == Some(vid)
                        && let Some(v) = self.mob_ref(vid)
                    {
                        let rank = v.mount.passengers.iter().position(|q| *q == Ent::Mob(pid)).unwrap_or(0);
                        r.pos = self.seat_of(v, rank, Some(&r));
                        if handles(v.kind) {
                            r.body_yaw = v.body_yaw;
                        }
                    }
                    r.fall_distance = 0.0;
                    self.mobs.list[i] = Some(r);
                    self.tick_passengers(pid);
                }
            }
        }
    }

    /// Sneaking → dismounts; otherwise seated on their seat.
    fn player_ride_tick(&mut self, vid: i32) {
        if self.sneaking {
            self.player_stop_riding();
            return;
        }
        let Some(v) = self.mob_ref(vid) else {
            self.mobs.mounts.riding.vehicle = None;
            return;
        };
        let rank = v.mount.passengers.iter().position(|q| *q == Ent::Player).unwrap_or(0);
        let seat = self.seat_of(v, rank, None);
        let key = Self::riding_stat_key(v.kind);
        let before = self.pos;
        if self.teleport.is_none() {
            self.pos = seat;
            self.last_good = seat;
        }
        // G5-F06: travel statistic.
        self.riding_stats(before, key);
    }

    /// Tick of the player outside a vehicle: remount delay.
    pub(crate) fn riding_player_tick(&mut self) {
        if self.mobs.mounts.riding.boarding_cooldown > 0 {
            self.mobs.mounts.riding.boarding_cooldown -= 1;
        }
        if let Some(v) = self.mobs.mounts.riding.vehicle
            && self.mob_ref(v).is_none_or(|m| m.removed)
        {
            self.player_stop_riding();
        }
        // Eyes under water (outside a bubble column) on an animal of the `dismounts_underwater` tag:
        // the player dismounts.
        if let Some(v) = self.mobs.mounts.riding.vehicle
            && let Some(k) = self.mob_ref(v).map(|m| m.kind)
            && dismounts_underwater(k)
        {
            let eye = super::super::mth::block_of([self.pos[0], self.player_eye_y(), self.pos[2]]);
            if self.mob_fluid(eye) == super::super::env::Fluid::Water && self.bname(eye) != "minecraft:bubble_column" {
                self.player_stop_riding();
            }
        }
    }

    /// Same rule for a passenger mob, called by its base tick when its eyes
    /// are in water outside a bubble column.
    pub(crate) fn mob_underwater_dismount(&mut self, m: &mut Mob) {
        let Some(vid) = m.mount.vehicle else { return };
        let Some(i) = self.mob_index(vid) else { return };
        if !self.mobs.list[i].as_ref().is_some_and(|v| dismounts_underwater(v.kind)) {
            return;
        }
        let Some(mut v) = self.mobs.list[i].take() else { return };
        self.mob_dismount_from(m, &mut v);
        self.mobs.list[i] = Some(v);
    }

    /// Mount piloted by the player: no server AI or movement; rotation taken from
    /// the player; the camel stands up if the player moves forward.
    pub(crate) fn tick_ridden(&mut self, m: &mut Mob) {
        // F12: strider dash.
        super::super::nether::strider::tick_boost(m);
        if super::super::recent::happy_ghast::tick_ridden(self, m) {
            // F14: the happy ghast catches up 8% of the yaw difference.
            m.vel = [0.0; 3];
            return;
        }
        if is_camel(m.kind) {
            if !super::camel::refuse_to_move(self, m) {
                m.yaw = self.yaw;
                m.pitch = self.pitch * 0.5;
            }
            let forward = self.mobs.mounts.riding.keys & 1 != 0 && self.mobs.mounts.riding.keys & 2 == 0;
            if forward && m.mount.last_pose_tick < 0 && !super::camel::in_transition(self, m) {
                super::camel::stand_up(self, m);
            }
        } else if super::super::sealife::is_nautilus(m.kind) {
            // F09: the nautilus turns by half the difference toward the rider's gaze.
            m.yaw += super::super::mth::wrap(self.yaw - m.yaw) * 0.5;
            m.pitch = self.pitch * 0.5;
        } else {
            m.yaw = self.yaw;
            m.pitch = self.pitch * 0.5;
        }
        m.body_yaw = m.yaw;
        m.head_yaw = m.yaw;
        if self.mobs.mounts.riding.keys & 1 == 0 {
            m.mount.gallop_counter = 0;
        }
        m.vel = [0.0; 3];
    }

    // ---------- Lent navigation ----------

    /// The steering rider borrows the mount's navigation and control for the duration of
    /// its goals (the game's navigation/movement control); returns the lender's id.
    pub(crate) fn borrow_vehicle_nav(&mut self, rider: &mut Mob) -> Option<i32> {
        let vid = rider.mount.vehicle?;
        let i = self.mob_index(vid)?;
        let v = self.mobs.list[i].as_mut()?;
        if Self::mob_controller(v) != Some(rider.id) {
            return None;
        }
        std::mem::swap(&mut rider.nav, &mut v.nav);
        std::mem::swap(&mut rider.move_ctl, &mut v.move_ctl);
        Some(vid)
    }

    pub(crate) fn return_vehicle_nav(&mut self, rider: &mut Mob, vid: Option<i32>) {
        let Some(vid) = vid else { return };
        let Some(i) = self.mob_index(vid) else { return };
        let Some(v) = self.mobs.list[i].as_mut() else { return };
        std::mem::swap(&mut rider.nav, &mut v.nav);
        std::mem::swap(&mut rider.move_ctl, &mut v.move_ctl);
    }

    // ---------- Network ----------

    /// List of passengers (network ids; the player by their id).
    pub(crate) fn passenger_ids(&self, v: &Mob) -> Vec<i32> {
        v.mount
            .passengers
            .iter()
            .map(|p| match p {
                Ent::Player => self.player_id,
                Ent::Mob(id) => *id,
            })
            .collect()
    }

    pub(crate) fn send_passengers(&self, v: &mut Mob) {
        let ids = self.passenger_ids(v);
        v.mount.sent_passengers = ids.clone();
        let _ = self.send(Event::Entity(EntityEvent::Passengers { vehicle: v.id, passengers: ids }));
    }

    /// End of tick: list changed → `set_passengers`.
    pub(crate) fn track_passengers(&self, v: &mut Mob) {
        let ids = self.passenger_ids(v);
        if ids != v.mount.sent_passengers {
            v.mount.sent_passengers = ids.clone();
            let _ = self.send(Event::Entity(EntityEvent::Passengers { vehicle: v.id, passengers: ids }));
        }
    }

    /// Pairing: passengers of the vehicle, vehicle of the passenger, mount attributes.
    pub(crate) fn pair_mount(&self, m: &mut Mob) {
        if !m.mount.passengers.is_empty() {
            self.send_passengers(m);
        }
        if let Some(vid) = m.mount.vehicle
            && let Some(v) = self.mob_ref(vid)
        {
            let ids = self.passenger_ids(v);
            let _ = self.send(Event::Entity(EntityEvent::Passengers { vehicle: vid, passengers: ids }));
        }
        // Any mob: syncable attributes.
        self.send_mount_attributes(m);
    }

    /// Attributes useful to the client for piloting (speed, jump, HP, walk, fall).
    /// Attributes synchronized with the client (pairing, then at every change): those the game
    /// marks as such, modeled or kept as is (scale…).
    pub(crate) fn send_mount_attributes(&self, m: &mut Mob) {
        self.send_mob_attributes(m, false);
    }

    /// Sending of the syncable attributes: all (`changed_only` false) or only those whose
    /// fingerprint changed since the last send (end-of-tick tracking, like the game).
    pub(crate) fn send_mob_attributes(&self, m: &mut Mob, changed_only: bool) {
        use rc_proto_v777::play::{Attribute, Modifier};
        let prints = super::super::mob::syncable_fingerprint(&m.attrs);
        let old = std::mem::replace(&mut m.track.attr_hash, prints);
        let keep = |i: usize| !changed_only || old[i] != prints[i];
        let Some(reg) = self.data.registry("minecraft:attribute") else { return };
        let index = |n: &str| reg.entries.iter().position(|e| e == n).map(|i| i as i32);
        let a = &m.attrs;
        let mut out = Vec::new();
        for (i, name) in super::super::mob::SYNCABLE_MODELED.into_iter().enumerate() {
            if !keep(i) {
                continue;
            }
            let (Some(id), Some(attr)) = (index(name), a.by_name(name)) else { continue };
            let modifiers = attr
                .mods
                .iter()
                .map(|(n, amount, op)| Modifier { id: (*n).into(), amount: *amount, operation: *op })
                .collect();
            out.push(Attribute { id, base: attr.base, modifiers });
        }
        let extra = if keep(super::super::mob::SYNCABLE_MODELED.len()) { &a.extra[..] } else { &[] };
        for e in extra {
            let name = e.get("id").and_then(rc_proto::nbt::Nbt::as_str).unwrap_or("");
            let name = if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") };
            if !super::super::mob::SYNCABLE_EXTRA.contains(&name.as_str()) {
                continue;
            }
            let Some(id) = index(&name) else { continue };
            let modifiers = crate::nbtio::list(e.get("modifiers"))
                .iter()
                .filter_map(|md| {
                    Some(Modifier {
                        id: md.get("id")?.as_str()?.to_owned(),
                        amount: md.get("amount").and_then(super::super::mob::f64_of).unwrap_or(0.0),
                        operation: match md.get("operation").and_then(rc_proto::nbt::Nbt::as_str) {
                            Some("add_multiplied_base") => 1,
                            Some("add_multiplied_total") => 2,
                            _ => 0,
                        },
                    })
                })
                .collect();
            let Some(base) = e.get("base").and_then(super::super::mob::f64_of) else { continue };
            out.push(Attribute { id, base, modifiers });
        }
        if !out.is_empty() {
            let _ = self.send(Event::Attributes { entity: m.id, attributes: out });
        }
    }

    /// Packets of the mounted player, read before the general decoding; true if handled here.
    pub(crate) fn mount_packet(&mut self, p: &[u8]) -> bool {
        let ids = self.peer.ids();
        let mut r = Reader::new(p);
        let Ok(id) = read_varint(&mut r) else { return false };
        let r = &mut r;
        if id == ids.move_vehicle_out {
            use rc_proto::Decode;
            let (Ok(x), Ok(y), Ok(z)) = (f64::decode(r), f64::decode(r), f64::decode(r)) else { return true };
            let (Ok(yaw), Ok(pitch)) = (f32::decode(r), f32::decode(r)) else { return true };
            let on_ground = bool::decode(r).unwrap_or(false);
            self.move_vehicle([x, y, z], yaw, pitch, on_ground);
            true
        } else if id == ids.player_input {
            // Keys remembered; the packet then follows the general handling (sneaking).
            self.mobs.mounts.riding.keys = r.u8().unwrap_or(0);
            false
        } else if (self.mobs.mounts.riding.vehicle.is_some() || self.deco.seat.is_some())
            && (id == ids.move_pos || id == ids.move_pos_rot)
        {
            // Passenger: only the rotation counts (in place).
            use rc_proto::Decode;
            let _ = (f64::decode(r), f64::decode(r), f64::decode(r));
            if id == ids.move_pos_rot
                && let (Ok(yaw), Ok(pitch)) = (f32::decode(r), f32::decode(r))
                && yaw.is_finite()
                && pitch.is_finite()
            {
                self.yaw = super::super::mth::wrap(yaw);
                self.pitch = pitch.clamp(-90.0, 90.0);
            }
            true
        } else {
            false
        }
    }

    /// Player command (`player_command`): 3 jump start, 4 end, 5 inventory.
    pub(crate) fn mount_command(&mut self, action: i32, data: i32) {
        let Some(vid) = self.mobs.mounts.riding.vehicle else { return };
        let Some(i) = self.mob_index(vid) else { return };
        let Some(mut v) = self.mobs.list[i].take() else { return };
        match action {
            3 if Self::player_controls(&v) && data > 0 && self.mount_can_jump(&v) => self.handle_start_jump(&mut v),
            5 => self.open_mount_menu(&mut v),
            _ => {}
        }
        self.mobs.list[i] = Some(v);
    }

    /// Saddled (camel: and not sitting).
    fn mount_can_jump(&self, v: &Mob) -> bool {
        saddled(v) && (!is_camel(v.kind) || !super::camel::refuse_to_move(self, v))
    }

    /// Equine → rears and jump sound; camel → dash.
    fn handle_start_jump(&mut self, v: &mut Mob) {
        // F09: nautilus dash (sound, "dash" state that arms the cooldown of 40).
        if super::super::sealife::is_nautilus(v.kind) {
            let s = super::super::sealife::nautilus::dash_sound(v);
            self.make_sound(v, s);
            super::super::sealife::nautilus::set_dash(v, true);
            return;
        }
        if is_camel(v.kind) {
            let s =
                if v.kind == Kind::Camel { "minecraft:entity.camel.dash" } else { "minecraft:entity.camel_husk.dash" };
            self.make_sound(v, s);
            // The change to true arms the cooldown (55) if it is empty.
            super::camel::set_dash(v, true);
            return;
        }
        v.mount.allow_stand_sliding = true;
        super::stand_if_possible(v);
        let s = match v.kind {
            Kind::Donkey => "minecraft:entity.donkey.jump",
            Kind::Mule => "minecraft:entity.mule.jump",
            Kind::SkeletonHorse if v.in_water => "minecraft:entity.skeleton_horse.jump_water",
            _ => "minecraft:entity.horse.jump",
        };
        self.mount_sound(v, s, 0.4, 1.0);
    }

    /// Movement with collisions, vertical difference ignored (the game always resets
    /// the vertical difference to 0), refused if the difference stays too large in free space.
    fn move_vehicle(&mut self, to: [f64; 3], yaw: f32, pitch: f32, on_ground: bool) {
        if to.iter().any(|v| !v.is_finite()) || !yaw.is_finite() || !pitch.is_finite() {
            return;
        }
        if self.teleport.is_some() || !self.player_ready() {
            return;
        }
        let Some(vid) = self.mobs.mounts.riding.vehicle else { return };
        let Some(i) = self.mob_index(vid) else { return };
        let Some(mut v) = self.mobs.list[i].take() else { return };
        if !Self::player_controls(&v) {
            self.mobs.list[i] = Some(v);
            return;
        }
        let to = [to[0].clamp(-3.0e7, 3.0e7), to[1].clamp(-2.0e7, 2.0e7), to[2].clamp(-3.0e7, 3.0e7)];
        let old = v.pos;
        let old_box = v.bbox();
        let good = self.mobs.mounts.riding.last_good;
        let d = [to[0] - good[0], to[1] - good[1], to[2] - good[2]];
        if self.on_climbable(&v) {
            v.fall_distance = 0.0;
        }
        self.mob_move(&mut v, d);
        let ex = to[0] - v.pos[0];
        let ez = to[2] - v.pos[2];
        // The game tests "> −0.5 or < 0.5": always true, the vertical difference is ignored.
        let moved = ex * ex + ez * ez;
        let fail = moved > 0.0625;
        let w = self.world();
        if fail && rc_physics::is_free(&w, &old_box) {
            v.pos = old;
            v.yaw = super::super::mth::wrap(yaw);
            v.pitch = super::super::mth::wrap(pitch);
            self.send_vehicle_back(&v);
            self.mobs.list[i] = Some(v);
            return;
        }
        v.pos = to;
        v.yaw = super::super::mth::wrap(yaw);
        v.pitch = super::super::mth::wrap(pitch);
        v.on_ground = on_ground;
        self.mobs.mounts.riding.last_good = v.pos;
        self.mobs.list[i] = Some(v);
    }

    /// `move_vehicle` packet (to the client): position and rotation imposed on the client.
    fn send_vehicle_back(&self, v: &Mob) {
        let _ = self.send(Event::MoveVehicle { pos: v.pos, yaw: v.yaw, pitch: v.pitch });
    }

    /// Same root vehicle (no push between rider and mount).
    pub(crate) fn same_root(&self, a: &Mob, b: &Mob) -> bool {
        let ra = self.root_of(a.id, a.mount.vehicle);
        let rb = self.root_of(b.id, b.mount.vehicle);
        ra == rb
    }

    /// Do the player and the mob share a root vehicle?
    pub(crate) fn player_same_root(&self, m: &Mob) -> bool {
        let Some(pv) = self.mobs.mounts.riding.vehicle else { return false };
        self.root_of(-1, Some(pv)) == self.root_of(m.id, m.mount.vehicle)
    }

    fn root_of(&self, id: i32, mut vehicle: Option<i32>) -> i32 {
        let mut root = id;
        let mut guard = 0;
        while let Some(v) = vehicle {
            root = v;
            vehicle = self.mob_ref(v).and_then(|m| m.mount.vehicle);
            guard += 1;
            if guard > 16 {
                break;
            }
        }
        root
    }
}

impl Server {
    /// Does another player (host included, LAN) ride the same root vehicle (or the
    /// same minecart) as the current player?
    pub(crate) fn vehicle_shared(&self) -> bool {
        let root = self.mobs.mounts.riding.vehicle.map(|v| self.root_of(-1, Some(v)));
        let cart = self.pcart.vehicle;
        self.lan.as_ref().is_some_and(|l| {
            l.guests.iter().filter_map(|g| g.ctx.as_ref()).any(|c| {
                c.ride.vehicle.is_some_and(|v| Some(self.root_of(-1, Some(v))) == root)
                    || (cart.is_some() && c.pcart.vehicle == cart)
            })
        })
    }

    /// Player file: mounted root vehicle (`RootVehicle`: UUID of the direct vehicle,
    /// root and its passengers), removed if it carries nothing or if the root also carries another
    /// player (the vehicle then stays in the world, never duplicated in the file).
    pub(crate) fn mount_player_nbt(&self, root: &mut rc_proto::nbt::Nbt) {
        use rc_proto::nbt::Nbt;
        if self.vehicle_shared() {
            crate::nbtio::remove(root, "RootVehicle");
            return;
        }
        let found = self.mobs.mounts.riding.vehicle.and_then(|vid| {
            let v = self.mob_ref(vid)?;
            let r = self.root_of(vid, v.mount.vehicle);
            Some((v.uuid, self.mob_ref(r)?))
        });
        // G5-F06: mounted minecart (always root here).
        let cart = self.pcart.vehicle.and_then(|id| self.cart_ref(id)).map(|c| (c.uuid, self.cart_to_nbt(c)));
        match (found, cart) {
            (Some((attach, r)), _) => {
                let e = self.save_with_passengers(r);
                let rv = Nbt::Compound(vec![("Attach".into(), crate::nbtio::uuid_nbt(attach)), ("Entity".into(), e)]);
                crate::nbtio::set(root, "RootVehicle", rv);
            }
            (None, Some((attach, e))) => {
                let rv = Nbt::Compound(vec![("Attach".into(), crate::nbtio::uuid_nbt(attach)), ("Entity".into(), e)]);
                crate::nbtio::set(root, "RootVehicle", rv);
            }
            (None, None) => crate::nbtio::remove(root, "RootVehicle"),
        }
    }

    /// Arrival: root vehicle from the player's file recreated, the player remounts (mounting an entity
    /// forced) on the one whose UUID is attached.
    pub(crate) fn load_root_vehicle(&mut self) {
        let Some(rv) = self.pdata.root.get("RootVehicle").cloned() else { return };
        let (Some(attach), Some(e)) = (crate::nbtio::uuid(rv.get("Attach")), rv.get("Entity")) else { return };
        let Some(u) = crate::nbtio::uuid(e.get("UUID")) else { return };
        // G5-F06: minecart recreated, the player remounts it.
        if e.get("id").and_then(rc_proto::nbt::Nbt::as_str).and_then(crate::minecart::CKind::from_name).is_some() {
            if self.carts.list.iter().flatten().any(|c| c.uuid == u) {
                return;
            }
            let Some(c) = self.cart_from_nbt(e) else { return };
            let id = c.id;
            self.cart_add(c);
            if u == attach {
                self.cart_ride_forced(id, crate::commands::ents::E::Player);
            }
            return;
        }
        if self.mobs.index_of_uuid(u).is_some() {
            return;
        }
        self.next_entity += 1;
        let id = self.next_entity;
        if self.adopt_with_passengers(id, e, None).is_none() {
            return;
        }
        let Some(i) = self.mobs.index_of_uuid(attach) else { return };
        let Some(mut v) = self.mobs.list[i].take() else { return };
        self.mobs.mounts.riding.boarding_cooldown = 0;
        let was = self.sneaking;
        self.sneaking = false;
        if !v.mount.passengers.contains(&Ent::Player) {
            self.player_start_riding(&mut v);
        }
        self.sneaking = was;
        self.mobs.list[i] = Some(v);
    }

    /// Departure of a LAN player (current player, already written to their file): like
    /// the removal of a player from the game, their root vehicle and all their passengers are removed
    /// without dropping anything if they are its only player, and their flying ender pearls (all
    /// dimensions) too; tracking erases them for the others.
    pub(crate) fn unload_with_leaving_player(&mut self) {
        let root = self.mobs.mounts.riding.vehicle.map(|v| self.root_of(-1, Some(v)));
        let cart = self.pcart.vehicle;
        if self.vehicle_shared() {
            // The vehicle stays with the other player: only the departing one dismounts. Player passengers
            // are indistinguishable in the list, removing just one gives the same list.
            if let Some(v) = self.mobs.mounts.riding.vehicle.take()
                && let Some(m) = self.mobs.get_mut(v)
                && let Some(k) = m.mount.passengers.iter().position(|p| *p == Ent::Player)
            {
                m.mount.passengers.remove(k);
            }
        } else {
            if let Some(r) = root {
                let mut stack = vec![r];
                while let Some(id) = stack.pop() {
                    let Some(m) = self.mobs.get_mut(id) else { continue };
                    m.removed = true;
                    stack.extend(m.mount.passengers.iter().filter_map(|p| match p {
                        Ent::Mob(i) => Some(*i),
                        Ent::Player => None,
                    }));
                }
            }
            if let Some(c) = cart.and_then(|id| self.cart_mut(id)) {
                c.removed = true;
            }
        }
        let me = Some(self.uuid);
        let mine = |p: &crate::projectiles::Proj| p.kind == crate::projectiles::PKind::EnderPearl && p.owner == me;
        for p in self.proj.list.iter_mut().filter(|p| mine(p)) {
            p.removed = true;
        }
        for w in &mut self.dims.parked {
            for p in w.proj.list.iter_mut().filter(|p| mine(p)) {
                p.removed = true;
            }
        }
    }

    /// World closing: the player's vehicle leaves with them (absent from the chunks).
    pub(crate) fn mount_unload_with_player(&mut self) {
        self.mobs.mounts.riding.unloading = self.mobs.mounts.riding.vehicle.is_some();
        self.pcart.unloading = self.pcart.vehicle.is_some(); // G5-F06
    }

    /// Mob excluded from the chunks: the player's vehicle (or passenger) at closing.
    pub(crate) fn leaves_with_player(&self, m: &Mob) -> bool {
        if !self.mobs.mounts.riding.unloading {
            return false;
        }
        let Some(pv) = self.mobs.mounts.riding.vehicle else { return false };
        self.root_of(-1, Some(pv)) == self.root_of(m.id, m.mount.vehicle)
    }
}

/// Registered passengers removed from the compound (`Passengers`).
pub fn take_passengers(e: &mut rc_proto::nbt::Nbt) -> Vec<rc_proto::nbt::Nbt> {
    let list = crate::nbtio::list(e.get("Passengers")).to_vec();
    crate::nbtio::remove(e, "Passengers");
    list
}

/// The horizontal escape vector.
fn escape_vector(collider: f64, colliding: f64, deg: f32) -> [f64; 3] {
    let dist = (collider + colliding + f64::from(1.0e-5f32)) / 2.0;
    let x = -sin(deg * DEG);
    let z = cos(deg * DEG);
    let s = x.abs().max(z.abs());
    [f64::from(x) * dist / f64::from(s), 0.0, f64::from(z) * dist / f64::from(s)]
}
