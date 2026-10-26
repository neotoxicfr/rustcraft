//! Network tracking of a minecart (interval 3, range 8 chunks, tracked
//! velocity): pairing, passengers, then relative or full position, rotation, velocity
//! (before the movement packet, the game's order) and data.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::{AddEntity, Delta, Meta, Move, PositionPath, PositionSync};

use super::Cart;
use crate::mobs::mob::Ent;
use crate::mobs::mth::pack_degrees;
use crate::mobs::track::lp_quantize;
use crate::server::Server;

/// Tracking interval of minecarts (default).
const INTERVAL: i32 = 3;
/// Tracking range, in chunks.
const RANGE: i32 = 8;

#[derive(Debug, Clone, Default)]
pub struct CTrack {
    pub seen: bool,
    base: [f64; 3],
    last_yaw: i8,
    last_pitch: i8,
    last_vel: [f64; 3],
    tick: i32,
    teleport_delay: i32,
    was_on_ground: bool,
    /// Passengers at the last send (network identifiers).
    pub sent_passengers: Vec<i32>,
    /// Hit received (the "hurt" marking requests sending the velocity): velocity sent again after the tracking.
    pub sync_velocity: bool,
}

fn unpack(b: i8) -> f32 {
    f32::from(b) * 360.0 / 256.0
}

fn enc(v: f64) -> i64 {
    rc_proto_v777::entity::delta_unit(v)
}

impl Server {
    pub(crate) fn cart_visible(&self, c: &Cart) -> bool {
        // Range raised by the passengers (a seated player: 32 chunks).
        let range = (f64::from(self.tracking_with_passengers(RANGE, &c.passengers)) * 16.0).min(f64::from(self.view) * 16.0);
        let dx = self.pos[0] - c.pos[0];
        let dz = self.pos[2] - c.pos[2];
        let ch = ((c.pos[0].floor() as i32) >> 4, (c.pos[2].floor() as i32) >> 4);
        dx * dx + dz * dz <= range * range && self.sent.contains(&ch) && !c.removed
    }

    /// Network identifiers of the passengers.
    pub(crate) fn cart_passenger_ids(&self, c: &Cart) -> Vec<i32> {
        c.passengers
            .iter()
            .map(|p| match p {
                Ent::Player => self.player_id,
                Ent::Mob(id) => *id,
            })
            .collect()
    }

    /// Non-default base entity data read from the kept fields (name, silence, no
    /// gravity, glow), then the minecart's.
    fn cart_non_default(&self, c: &Cart) -> Vec<(u8, Meta)> {
        let n = &c.nbt;
        let flag = |k: &str| crate::nbtio::int(n, k).is_some_and(|v| v != 0);
        let mut out = Vec::new();
        let mut shared = 0i8;
        if flag("Glowing") {
            shared |= 0x40;
        }
        if shared != 0 {
            out.push((0, Meta::Byte(shared)));
        }
        if let Some(a) = crate::nbtio::int(n, "Air").filter(|&a| a != 300) {
            out.push((1, Meta::Int(a as i32)));
        }
        if let Some(name) = n.get("CustomName") {
            out.push((2, Meta::Text(Some(name.clone()))));
        }
        if flag("CustomNameVisible") {
            out.push((3, Meta::Bool(true)));
        }
        if c.silent {
            out.push((4, Meta::Bool(true)));
        }
        if c.no_gravity {
            out.push((5, Meta::Bool(true)));
        }
        if let Some(t) = crate::nbtio::int(n, "TicksFrozen").filter(|&t| t > 0) {
            out.push((7, Meta::Int(t as i32)));
        }
        for (i, v, _) in &c.meta {
            // 7 (freezing) already read from the kept fields.
            if *i != 0 && *i != 7 && *v != c.default_meta(*i) {
                out.push((*i, v.clone()));
            }
        }
        out
    }

    /// Pairing: add, non-default data, passengers.
    pub(crate) fn cart_pair(&self, c: &mut Cart) {
        let add = AddEntity {
            id: c.id,
            uuid: c.uuid,
            kind: self.entity_kind_id(c.kind.name()),
            pos: c.pos,
            velocity: lp_quantize(c.vel),
            pitch: unpack(pack_degrees(c.pitch)),
            yaw: unpack(pack_degrees(c.yaw)),
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let values = self.cart_non_default(c);
        for s in &mut c.meta {
            s.2 = false;
        }
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id: c.id, values }));
        }
        let ids = self.cart_passenger_ids(c);
        if !ids.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Passengers { vehicle: c.id, passengers: ids.clone() }));
        }
        let t = &mut c.track;
        t.sent_passengers = ids;
        t.seen = true;
        t.base = c.pos;
        t.last_yaw = pack_degrees(c.yaw);
        t.last_pitch = pack_degrees(c.pitch);
        t.last_vel = c.vel;
        t.was_on_ground = c.on_ground;
    }

    /// End of tick.
    pub(crate) fn cart_send_changes(&self, c: &mut Cart) {
        let visible = self.cart_visible(c);
        if visible && !c.track.seen {
            self.cart_pair(c);
        } else if !visible && c.track.seen {
            let _ = self.send(Event::Entity(EntityEvent::Remove(vec![c.id])));
            c.track.seen = false;
        }
        if !c.track.seen {
            c.take_dirty();
            c.track.tick += 1;
            c.track.sync_velocity = false;
            return;
        }
        let ids = self.cart_passenger_ids(c);
        if ids != c.track.sent_passengers {
            let _ = self.send(Event::Entity(EntityEvent::Passengers { vehicle: c.id, passengers: ids.clone() }));
            c.track.sent_passengers = ids;
        }
        let dirty = c.meta.iter().any(|s| s.2);
        if c.needs_sync || c.track.tick % INTERVAL == 0 || dirty {
            let yaw = pack_degrees(c.yaw);
            let pitch = pack_degrees(c.pitch);
            let rot = (i32::from(yaw) - i32::from(c.track.last_yaw)).abs() >= 1
                || (i32::from(pitch) - i32::from(c.track.last_pitch)).abs() >= 1;
            c.track.teleport_delay += 1;
            let d = [c.pos[0] - c.track.base[0], c.pos[1] - c.track.base[1], c.pos[2] - c.track.base[2]];
            let changed = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= f64::from(7.629_394_5e-6f32);
            let send_pos = changed || c.track.tick % 60 == 0;
            let full = c.track.teleport_delay > 400 || c.track.was_on_ground != c.on_ground;
            // Movement packet chosen first (without sending it), velocity sent before it.
            enum Pk {
                Sync,
                Move(Move),
                None,
            }
            let (pk, sent_pos, sent_rot) = if full {
                c.track.was_on_ground = c.on_ground;
                c.track.teleport_delay = 0;
                (Pk::Sync, true, true)
            } else if send_pos {
                let e = [
                    enc(c.pos[0]) - enc(c.track.base[0]),
                    enc(c.pos[1]) - enc(c.track.base[1]),
                    enc(c.pos[2]) - enc(c.track.base[2]),
                ];
                if e.iter().any(|v| !(-32768..=32767).contains(v)) {
                    (Pk::Sync, true, true)
                } else {
                    let mv = Move {
                        id: c.id,
                        on_ground: c.on_ground,
                        delta: Some(Delta::Linear([e[0] as i16, e[1] as i16, e[2] as i16])),
                        rot: rot.then(|| (unpack(yaw), unpack(pitch))),
                    };
                    (Pk::Move(mv), true, rot)
                }
            } else if rot {
                let mv = Move { id: c.id, on_ground: c.on_ground, delta: None, rot: Some((unpack(yaw), unpack(pitch))) };
                (Pk::Move(mv), false, true)
            } else {
                (Pk::None, false, false)
            };
            let v = c.vel;
            let dv = [v[0] - c.track.last_vel[0], v[1] - c.track.last_vel[1], v[2] - c.track.last_vel[2]];
            let diff = dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2];
            if diff > 1.0e-7 || (diff > 0.0 && v == [0.0; 3]) {
                c.track.last_vel = v;
                let _ = self.send(Event::Entity(EntityEvent::Motion { id: c.id, velocity: lp_quantize(v) }));
            }
            match pk {
                Pk::Sync => {
                    let s = PositionSync {
                        id: c.id,
                        path: PositionPath { linear: true, steps: vec![(c.pos, 0)] },
                        yaw: c.yaw,
                        pitch: c.pitch,
                        on_ground: c.on_ground,
                    };
                    let _ = self.send(Event::Entity(EntityEvent::Sync(s)));
                }
                Pk::Move(mv) => {
                    let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                }
                Pk::None => {}
            }
            let values = c.take_dirty();
            if !values.is_empty() {
                let _ = self.send(Event::Entity(EntityEvent::Data { id: c.id, values }));
            }
            if sent_pos {
                c.track.base = c.pos;
            }
            if sent_rot {
                c.track.last_yaw = yaw;
                c.track.last_pitch = pitch;
            }
            c.needs_sync = false;
        }
        c.track.tick += 1;
        if c.track.sync_velocity {
            c.track.sync_velocity = false;
            let _ = self.send(Event::Entity(EntityEvent::Motion { id: c.id, velocity: lp_quantize(c.vel) }));
        }
    }

    /// Entity event (70: TNT fuse).
    pub(crate) fn cart_event(&self, c: &Cart, ev: i8) {
        if c.track.seen {
            let _ = self.send(Event::Entity(EntityEvent::Event { id: c.id, event: ev }));
        }
    }
}

