//! Network tracking of a mob, like the official server: pairing
//! (add, metadata, equipment), then each tick the changes: relative position
//! (1/4096 of a block) or full sync, rotation, head, velocity, data.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::{AddEntity, Delta, Move, PositionPath, PositionSync};

use super::mob::Mob;
use super::mth::pack_degrees;
use crate::server::Server;

#[derive(Debug, Clone, Default)]
pub struct Tracker {
    pub seen: bool,
    base: [f64; 3],
    last_yaw: i8,
    last_pitch: i8,
    last_head: i8,
    last_vel: [f64; 3],
    tick: i32,
    teleport_delay: i32,
    was_on_ground: bool,
    /// Velocity to send back to the client and to the owner (hits received).
    pub sync_velocity: bool,
    /// F04: passenger at the last send.
    pub was_riding: bool,
    /// Pushed object (immediate send requested: explosion…): position and velocity sent at the next
    /// tracking, even outside the interval.
    pub needs_sync: bool,
    /// Fingerprints of the syncable attributes at the last send.
    pub attr_hash: super::mob::AttrPrints,
}

/// Tracking range of the player entity type (chunks).
const PLAYER_TRACKING: i32 = 32;

fn unpack(b: i8) -> f32 {
    f32::from(b) * 360.0 / 256.0
}

fn enc(v: f64) -> i64 {
    rc_proto_v777::entity::delta_unit(v)
}

/// Velocity as the client decodes it.
pub fn lp_quantize(v: [f64; 3]) -> [f64; 3] {
    let v = v.map(|x| if x.is_nan() { 0.0 } else { x.clamp(-1.717_986_918_3e10, 1.717_986_918_3e10) });
    let m = v[0].abs().max(v[1].abs()).max(v[2].abs());
    if m < 3.051_944_088_384_301e-5 {
        return [0.0; 3];
    }
    let scale = m.ceil();
    v.map(|x| {
        let q = ((x / scale * 0.5 + 0.5) * 32766.0).round().min(32766.0);
        (q * 2.0 / 32766.0 - 1.0) * scale
    })
}

impl Server {
    /// Does the player track this mob (tracking range and chunk sent)?
    fn tracks(&self, m: &Mob) -> bool {
        let range = (f64::from(self.effective_tracking(m)) * 16.0).min(f64::from(self.view) * 16.0);
        let dx = self.pos[0] - m.pos[0];
        let dz = self.pos[2] - m.pos[2];
        let c = ((m.pos[0].floor() as i32) >> 4, (m.pos[2].floor() as i32) >> 4);
        dx * dx + dz * dz <= range * range && self.sent.contains(&c) && !m.removed
    }

    /// Tracking range (chunks): the type's, or that of a direct or indirect passenger if
    /// larger (a mounted player brings its mount to 32 chunks).
    pub(crate) fn effective_tracking(&self, m: &Mob) -> i32 {
        self.tracking_with_passengers(m.kind.info().tracking, &m.mount.passengers)
    }

    /// `base` range (chunks) raised by direct and indirect passengers (mobs, minecarts).
    pub(crate) fn tracking_with_passengers(&self, base: i32, passengers: &[super::mob::Ent]) -> i32 {
        let mut range = base;
        // Common case: no passenger, nothing to walk (nor allocation).
        if passengers.is_empty() {
            return range;
        }
        let mut stack: Vec<super::mob::Ent> = passengers.to_vec();
        let mut guard = 0;
        while let Some(p) = stack.pop() {
            guard += 1;
            if guard > 64 {
                break;
            }
            match p {
                super::mob::Ent::Player => range = range.max(PLAYER_TRACKING),
                super::mob::Ent::Mob(id) => {
                    if let Some(o) = self.mob_ref(id) {
                        range = range.max(o.kind.info().tracking);
                        stack.extend(o.mount.passengers.iter().copied());
                    }
                }
            }
        }
        range
    }

    /// Pairing: add, non-default data, equipment.
    pub(crate) fn pair(&self, m: &mut Mob) {
        m.sync_data();
        let add = AddEntity {
            id: m.id,
            uuid: m.uuid,
            kind: self.entity_kind_id(m.kind.info().name),
            pos: m.pos,
            velocity: lp_quantize(m.vel),
            pitch: unpack(pack_degrees(m.pitch)),
            yaw: unpack(pack_degrees(m.yaw)),
            head_yaw: unpack(pack_degrees(m.head_yaw)),
            // F14: emerging warden (1).
            data: super::recent::add_entity_data(m),
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let values = m.non_default();
        m.take_dirty();
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id: m.id, values }));
        }
        let slots: rc_proto_v777::entity::Equipment =
            m.equipment.iter().enumerate().filter_map(|(i, s)| Some((i as u8, self.net(Some(s.as_ref()?))))).collect();
        if !slots.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Equipment { id: m.id, slots }));
        }
        // Active effects: never sent at pairing (only a rider receives them).
        self.pair_mount(m); // F04: passengers, mount attributes
        let t = &mut m.track;
        t.seen = true;
        t.base = m.pos;
        t.last_yaw = pack_degrees(m.yaw);
        t.last_pitch = pack_degrees(m.pitch);
        t.last_head = pack_degrees(m.head_yaw);
        t.last_vel = m.vel;
        t.was_on_ground = m.on_ground;
    }

    pub(crate) fn unpair(&self, m: &mut Mob) {
        if m.track.seen {
            let _ = self.send(Event::Entity(EntityEvent::Remove(vec![m.id])));
            m.track.seen = false;
        }
    }

    pub(crate) fn entity_kind_id(&self, name: &str) -> i32 {
        self.data
            .registry("minecraft:entity_type")
            .and_then(|r| r.entries.iter().position(|e| e == name))
            .map_or(0, |i| i as i32)
    }

    /// End of tick (the chunk map tick → sending the changes).
    pub(crate) fn send_changes(&self, m: &mut Mob) {
        let visible = self.tracks(m);
        if visible && !m.track.seen {
            self.pair(m);
        } else if !visible && m.track.seen {
            self.unpair(m);
        }
        super::wither::boss::bar_track(self, m); // F16: boss bar
        if !m.track.seen {
            m.sync_data();
            m.take_dirty();
            m.track.tick += 1;
            return;
        }
        m.sync_data();
        self.track_passengers(m); // F04
        // F10: the allay is tracked every 2 ticks.
        let interval = if m.kind == super::species::Kind::Allay { 2 } else { 3 };
        let id = m.id;
        let dirty_data = m.data.iter().any(|s| s.dirty);
        let t_tick = m.track.tick;
        if m.needs_sync || t_tick % interval == 0 || dirty_data {
            let yaw = pack_degrees(m.yaw);
            let pitch = pack_degrees(m.pitch);
            let rot = (i32::from(yaw) - i32::from(m.track.last_yaw)).abs() >= 1
                || (i32::from(pitch) - i32::from(m.track.last_pitch)).abs() >= 1;
            m.track.teleport_delay += 1;
            let pos = m.pos;
            let d = [pos[0] - m.track.base[0], pos[1] - m.track.base[1], pos[2] - m.track.base[2]];
            let changed = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= f64::from(7.629_394_5e-6f32);
            // F04: a passenger sends only its rotation; on dismount, full sync.
            let riding = m.mount.vehicle.is_some();
            let was_riding = std::mem::replace(&mut m.track.was_riding, riding);
            let send_pos = !riding && (changed || t_tick % 60 == 0);
            // F14: precise position required (immobilised happy ghast): full sync.
            let precise = super::recent::requires_precise_position(m);
            let full = !riding
                && (precise || m.track.teleport_delay > 400 || m.track.was_on_ground != m.on_ground || was_riding);
            let mut sent_pos = false;
            let mut sent_rot = false;
            if full {
                m.track.was_on_ground = m.on_ground;
                m.track.teleport_delay = 0;
                self.sync_full(m);
                sent_pos = true;
                sent_rot = true;
            } else if send_pos {
                let e = [
                    enc(pos[0]) - enc(m.track.base[0]),
                    enc(pos[1]) - enc(m.track.base[1]),
                    enc(pos[2]) - enc(m.track.base[2]),
                ];
                if e.iter().any(|v| !(-32768..=32767).contains(v)) {
                    self.sync_full(m);
                    sent_rot = true;
                } else {
                    let mv = Move {
                        id,
                        on_ground: m.on_ground,
                        delta: Some(Delta::Linear([e[0] as i16, e[1] as i16, e[2] as i16])),
                        rot: rot.then(|| (unpack(yaw), unpack(pitch))),
                    };
                    let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                    sent_rot = rot;
                }
                sent_pos = true;
            } else if rot {
                let mv = Move { id, on_ground: m.on_ground, delta: None, rot: Some((unpack(yaw), unpack(pitch))) };
                let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                sent_rot = true;
            }
            // Velocity (mobs: tracked position difference).
            let v = m.vel;
            let dv = [v[0] - m.track.last_vel[0], v[1] - m.track.last_vel[1], v[2] - m.track.last_vel[2]];
            let diff = dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2];
            // Bat: type without velocity tracking, except forced synchronisation.
            // F16: nor the wither.
            let deltas =
                (m.kind != super::species::Kind::Bat && !super::wither::no_track_deltas(m.kind)) || m.needs_sync;
            if deltas && (diff > 1.0e-7 || (diff > 0.0 && v == [0.0; 3])) {
                m.track.last_vel = v;
                let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(v) }));
            }
            let values = m.take_dirty();
            if !values.is_empty() {
                let _ = self.send(Event::Entity(EntityEvent::Data { id, values }));
            }
            // Then only the changed syncable attributes (base or modifiers).
            if super::mob::syncable_fingerprint(&m.attrs) != m.track.attr_hash {
                self.send_mob_attributes(m, true);
            }
            if sent_pos {
                m.track.base = pos;
            }
            if sent_rot {
                m.track.last_yaw = yaw;
                m.track.last_pitch = pitch;
            }
            let head = pack_degrees(m.head_yaw);
            if (i32::from(head) - i32::from(m.track.last_head)).abs() >= 1 {
                let _ = self.send(Event::Entity(EntityEvent::Head { id, yaw: unpack(head) }));
                m.track.last_head = head;
            }
            m.needs_sync = false;
        }
        m.track.tick += 1;
        if m.track.sync_velocity {
            m.track.sync_velocity = false;
            let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(m.vel) }));
        }
    }

    /// Tracking of an item without head or rotation (dropped item, orb): same rule as
    /// mobs, interval specific to the type (20 for items and orbs).
    pub(crate) fn track_object(
        &self,
        t: &mut Tracker,
        id: i32,
        pos: [f64; 3],
        vel: [f64; 3],
        on_ground: bool,
        interval: i32,
    ) {
        self.track_turned(t, id, pos, vel, on_ground, interval, [0.0; 2]);
    }

    /// Same tracking for an object rotated once and for all (armour stand, mannequin):
    /// the full sync carries its rotation (`rot`: yaw, pitch).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn track_turned(
        &self,
        t: &mut Tracker,
        id: i32,
        pos: [f64; 3],
        vel: [f64; 3],
        on_ground: bool,
        interval: i32,
        rot: [f32; 2],
    ) {
        if !t.seen {
            t.seen = true;
            t.base = pos;
            t.last_vel = vel;
            t.was_on_ground = on_ground;
        }
        let tick = t.tick;
        if tick % interval == 0 || t.needs_sync {
            t.needs_sync = false;
            t.teleport_delay += 1;
            let d = [pos[0] - t.base[0], pos[1] - t.base[1], pos[2] - t.base[2]];
            let changed = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= f64::from(7.629_394_5e-6f32);
            let send_pos = changed || tick % 60 == 0;
            let full = t.teleport_delay > 400 || t.was_on_ground != on_ground;
            let sync = |s: &Server| {
                let p = PositionSync {
                    id,
                    path: PositionPath { linear: true, steps: vec![(pos, 0)] },
                    yaw: rot[0],
                    pitch: rot[1],
                    on_ground,
                };
                let _ = s.send(Event::Entity(EntityEvent::Sync(p)));
            };
            if full {
                t.was_on_ground = on_ground;
                t.teleport_delay = 0;
                sync(self);
                t.base = pos;
            } else if send_pos {
                let e = [enc(pos[0]) - enc(t.base[0]), enc(pos[1]) - enc(t.base[1]), enc(pos[2]) - enc(t.base[2])];
                if e.iter().any(|v| !(-32768..=32767).contains(v)) {
                    sync(self);
                } else {
                    let mv = Move {
                        id,
                        on_ground,
                        delta: Some(Delta::Linear([e[0] as i16, e[1] as i16, e[2] as i16])),
                        rot: None,
                    };
                    let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                }
                t.base = pos;
            }
            let dv = [vel[0] - t.last_vel[0], vel[1] - t.last_vel[1], vel[2] - t.last_vel[2]];
            let diff = dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2];
            if diff > 1.0e-7 || (diff > 0.0 && vel == [0.0; 3]) {
                t.last_vel = vel;
                let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(vel) }));
            }
        }
        t.tick += 1;
        // Hit received (velocity send requested): velocity sent back as is after the tracking.
        if t.sync_velocity {
            t.sync_velocity = false;
            let _ = self.send(Event::Entity(EntityEvent::Motion { id, velocity: lp_quantize(vel) }));
        }
    }

    /// F14: position sync sent right away, coding base re-aligned.
    pub(crate) fn sync_position_now(&self, m: &mut Mob) {
        if m.track.seen {
            m.track.base = m.pos;
            self.sync_full(m);
        }
    }

    fn sync_full(&self, m: &Mob) {
        let s = PositionSync {
            id: m.id,
            path: PositionPath { linear: true, steps: vec![(m.pos, 0)] },
            yaw: m.yaw,
            pitch: m.pitch,
            on_ground: m.on_ground,
        };
        let _ = self.send(Event::Entity(EntityEvent::Sync(s)));
    }
}
