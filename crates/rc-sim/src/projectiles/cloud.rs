//! Effect cloud of lingering potions: wait, radius that shrinks
//! each tick and at each victim, effects applied every 5 ticks (instant ones at
//! half power), reapplication delay per victim.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::{AddEntity, Meta, Particle};

use crate::effects::Inst;
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

#[derive(Debug, Clone)]
pub struct Cloud {
    pub id: i32,
    pub uuid: u128,
    pub pos: [f64; 3],
    pub owner: Option<u128>,
    pub radius: f32,
    pub radius_on_use: f32,
    pub radius_per_tick: f32,
    pub duration: i32,
    pub duration_on_use: i32,
    pub wait_time: i32,
    pub reapplication: i32,
    pub age: i32,
    /// Stack the effects come from (`potion_contents`).
    pub potion: Option<Stack>,
    pub duration_scale: f32,
    /// F15: own particle (`custom_particle`: kind, power), dragon's breath.
    pub custom_particle: Option<(String, f32)>,
    /// Victims (network identifier) and tick when they become susceptible again.
    pub victims: Vec<(i32, i32)>,
    pub waiting: bool,
    pub removed: bool,
    pub seen: bool,
    sent_radius: f32,
    sent_waiting: bool,
}

impl Cloud {
    pub fn new(id: i32, pos: [f64; 3]) -> Cloud {
        let mut rng = crate::mobs::jrand::JRand::from_time(i64::from(id) ^ 0x4145);
        let uuid = (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128;
        Cloud {
            id,
            uuid,
            pos,
            owner: None,
            radius: 3.0,
            radius_on_use: 0.0,
            radius_per_tick: 0.0,
            duration: -1,
            duration_on_use: 0,
            wait_time: 20,
            reapplication: 20,
            age: 0,
            potion: None,
            duration_scale: 1.0,
            custom_particle: None,
            victims: Vec::new(),
            waiting: false,
            removed: false,
            seen: false,
            sent_radius: 3.0,
            sent_waiting: false,
        }
    }

    fn set_radius(&mut self, r: f32) {
        self.radius = r.clamp(0.0, 32.0);
    }

    fn effects(&self) -> Vec<Inst> {
        self.potion.as_ref().map(|s| crate::potions::stack_effects(s, self.duration_scale)).unwrap_or_default()
    }

    /// Box (width 2 × radius, height 0.5).
    pub fn bbox(&self) -> rc_physics::Aabb {
        let r = f64::from(self.radius);
        rc_physics::Aabb {
            min: [self.pos[0] - r, self.pos[1], self.pos[2] - r],
            max: [self.pos[0] + r, self.pos[1] + 0.5, self.pos[2] + r],
        }
    }

    /// Particle colour (opaque).
    fn color(&self) -> u32 {
        let c = self.potion.as_ref().map_or(-13083194, super::hits::potion_color);
        (c as u32) | 0xFF00_0000
    }

    pub fn to_nbt(&self) -> Nbt {
        let mut f: Vec<(String, Nbt)> = vec![
            ("id".into(), Nbt::String("minecraft:area_effect_cloud".into())),
            ("Pos".into(), Nbt::List(self.pos.iter().map(|&v| Nbt::Double(v)).collect())),
            ("Age".into(), Nbt::Int(self.age)),
            ("Duration".into(), Nbt::Int(self.duration)),
            ("WaitTime".into(), Nbt::Int(self.wait_time)),
            ("ReapplicationDelay".into(), Nbt::Int(self.reapplication)),
            ("DurationOnUse".into(), Nbt::Int(self.duration_on_use)),
            ("RadiusOnUse".into(), Nbt::Float(self.radius_on_use)),
            ("RadiusPerTick".into(), Nbt::Float(self.radius_per_tick)),
            ("Radius".into(), Nbt::Float(self.radius)),
            ("UUID".into(), super::save::uuid_nbt(self.uuid)),
        ];
        if let Some(o) = self.owner {
            f.push(("Owner".into(), super::save::uuid_nbt(o)));
        }
        if let Some(c) = self.potion.as_ref().and_then(|s| crate::items::component(s, "minecraft:potion_contents")) {
            f.push(("potion_contents".into(), c.clone()));
        }
        if self.duration_scale != 1.0 {
            f.push(("potion_duration_scale".into(), Nbt::Float(self.duration_scale)));
        }
        if let Some((ty, power)) = &self.custom_particle {
            // Power omitted when it is 1 (the format's default value).
            let mut p = vec![("type".into(), Nbt::String(ty.clone()))];
            if *power != 1.0 {
                p.push(("power".into(), Nbt::Float(*power)));
            }
            f.push(("custom_particle".into(), Nbt::Compound(p)));
        }
        Nbt::Compound(f)
    }

    pub fn from_nbt(id: i32, e: &Nbt) -> Cloud {
        let pos = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let mut c = Cloud::new(id, pos);
        let i = |k: &str, d: i32| crate::nbtio::int(e, k).map_or(d, |v| v as i32);
        let f = |k: &str, d: f32| match e.get(k) {
            Some(Nbt::Float(v)) => *v,
            Some(Nbt::Double(v)) => *v as f32,
            _ => d,
        };
        c.age = i("Age", 0);
        c.duration = i("Duration", -1);
        c.wait_time = i("WaitTime", 20);
        c.reapplication = i("ReapplicationDelay", 20);
        c.duration_on_use = i("DurationOnUse", 0);
        c.radius_on_use = f("RadiusOnUse", 0.0);
        c.radius_per_tick = f("RadiusPerTick", 0.0);
        c.set_radius(f("Radius", 3.0));
        c.sent_radius = c.radius;
        c.owner = super::save::uuid_of(e.get("Owner"));
        if let Some(u) = super::save::uuid_of(e.get("UUID")) {
            c.uuid = u;
        }
        if let Some(p) = e.get("potion_contents") {
            let mut s = Stack { id: "minecraft:potion".into(), count: 1, components: None };
            crate::items::set_component(&mut s, "minecraft:potion_contents", p.clone());
            c.potion = Some(s);
        }
        c.duration_scale = f("potion_duration_scale", 1.0);
        c.custom_particle = e.get("custom_particle").and_then(|p| {
            let ty = match p {
                Nbt::String(s) => s.clone(),
                _ => p.get("type").and_then(Nbt::as_str)?.to_owned(),
            };
            let power = match p.get("power") {
                Some(Nbt::Float(v)) => *v,
                Some(Nbt::Double(v)) => *v as f32,
                _ => 1.0,
            };
            Some((ty, power))
        });
        c
    }
}

impl Server {
    pub(crate) fn cloud_add(&mut self, mut c: Cloud) {
        self.cloud_pair(&mut c);
        self.proj.clouds.push(c);
    }

    fn cloud_pair(&self, c: &mut Cloud) {
        let add = AddEntity {
            id: c.id,
            uuid: c.uuid,
            kind: self.entity_kind_id("minecraft:area_effect_cloud"),
            pos: c.pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let particle = self.cloud_particle(c);
        // Particle alone (serializer 16), not a list.
        let values = vec![(8, Meta::Float(c.radius)), (9, Meta::Bool(c.waiting)), (10, Meta::Particle(particle))];
        let _ = self.send(Event::Entity(EntityEvent::Data { id: c.id, values }));
        c.seen = true;
        c.sent_radius = c.radius;
        c.sent_waiting = c.waiting;
    }

    fn cloud_particle(&self, c: &Cloud) -> Particle {
        let name = c.custom_particle.as_ref().map_or("minecraft:entity_effect", |p| p.0.as_str());
        let kind = self
            .data
            .registry("minecraft:particle_type")
            .and_then(|r| r.entries.iter().position(|e| e == name))
            .unwrap_or(0) as i32;
        match &c.custom_particle {
            // F15: dragon's breath (power).
            Some((_, power)) => Particle { kind, color: 0, state: 0, item: -1, scale: *power, color2: 0, vib: None },
            None => Particle { kind, color: c.color(), state: 0, item: -1, scale: 1.0, color2: 0, vib: None },
        }
    }

    /// F15: cloud removed (`discard`, flame of the sitting dragon).
    pub(crate) fn cloud_discard(&mut self, id: i32) {
        if let Some(c) = self.proj.clouds.iter_mut().find(|c| c.id == id) {
            c.removed = true;
        }
    }

    /// Tick of the clouds then modified data.
    pub(crate) fn clouds_tick(&mut self) {
        let mut clouds = std::mem::take(&mut self.proj.clouds);
        for c in clouds.iter_mut().filter(|c| !c.removed) {
            if !c.seen {
                self.cloud_pair(c);
            }
            c.age += 1;
            self.cloud_tick(c);
            if c.removed {
                continue;
            }
            let mut values = Vec::new();
            if c.radius != c.sent_radius {
                c.sent_radius = c.radius;
                values.push((8, Meta::Float(c.radius)));
            }
            if c.waiting != c.sent_waiting {
                c.sent_waiting = c.waiting;
                values.push((9, Meta::Bool(c.waiting)));
            }
            if !values.is_empty() {
                let _ = self.send(Event::Entity(EntityEvent::Data { id: c.id, values }));
            }
        }
        let gone: Vec<i32> = clouds.iter().filter(|c| c.removed).map(|c| c.id).collect();
        clouds.retain(|c| !c.removed);
        clouds.extend(std::mem::take(&mut self.proj.clouds));
        self.proj.clouds = clouds;
        if !gone.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Remove(gone)));
        }
    }

    fn cloud_tick(&mut self, c: &mut Cloud) {
        // The game's tick counter: age before this tick + 1 (incremented before the tick).
        let t = c.age;
        if c.duration != -1 && t - c.wait_time >= c.duration {
            c.removed = true;
            return;
        }
        let should_wait = t < c.wait_time;
        c.waiting = should_wait;
        if should_wait {
            return;
        }
        let mut radius = c.radius;
        if c.radius_per_tick != 0.0 {
            radius += c.radius_per_tick;
            if radius < 0.5 {
                c.removed = true;
                return;
            }
            c.set_radius(radius);
        }
        if t % 5 != 0 {
            return;
        }
        c.victims.retain(|v| t < v.1);
        let effects = c.effects();
        if effects.is_empty() {
            c.victims.clear();
            return;
        }
        let by = c.owner.and_then(|u| {
            if u == self.uuid { Some(Ent::Player) } else { self.mobs.get_uuid(u).map(|m| Ent::Mob(m.id)) }
        });
        for (e, pos) in self.living_in(&c.bbox()) {
            let nid = self.ent_net_id(e);
            if c.victims.iter().any(|v| v.0 == nid) {
                continue;
            }
            let affected = match e {
                Ent::Player => true,
                Ent::Mob(id) => self
                    .mob_ref(id)
                    .is_some_and(|m| effects.iter().any(|x| crate::mobs::effects::can_affect(m.kind, &x.id))),
            };
            if !affected {
                continue;
            }
            let (xd, zd) = (pos[0] - c.pos[0], pos[2] - c.pos[2]);
            if xd * xd + zd * zd > f64::from(radius * radius) {
                continue;
            }
            c.victims.push((nid, t + c.reapplication));
            for eff in &effects {
                if crate::lootfn::instantaneous(&eff.id) {
                    // Indirect magic (source: the cloud, author: its owner): knockback from
                    // the cloud for any victim; difficulty and named author for the player.
                    self.instant_on(e, &eff.id, eff.amp, 0.5, Some(by), Some(c.pos));
                } else {
                    self.give_ent_effect(e, eff.clone(), 1.0);
                }
            }
            if c.radius_on_use != 0.0 {
                radius += c.radius_on_use;
                if radius < 0.5 {
                    c.removed = true;
                    return;
                }
                c.set_radius(radius);
            }
            if c.duration_on_use != 0 && c.duration != -1 {
                c.duration += c.duration_on_use;
                if c.duration <= 0 {
                    c.removed = true;
                    return;
                }
            }
        }
    }
}
