//! Items on the ground and experience orbs (`server-gameplay.md` § 1.3, 5.6): appearance (block
//! loot, thrown items, container contents), simple game physics (gravity, drag,
//! bounce), merging, pickup (`take_item_entity`), lifetime of 6000 ticks; loot of
//! broken blocks, tool wear, experience from ores.

use rc_client::{EntityEvent, Event};
use rc_physics::Aabb;
use rc_physics::body::{Body, Follow};
use rc_proto_v777::entity::{AddEntity, Meta};

use crate::items::same;
use crate::menus::inventory_add;
use crate::mobs::recent::vibration::Src;
use crate::player::Stack;
use crate::server::Server;

const ITEM_SIZE: f64 = 0.25;
const ORB_SIZE: f64 = 0.5;
const LIFETIME: i32 = 6000;

/// Physics of a server item or orb (authoritative instance).
fn server_body(pos: [f64; 3], vel: [f64; 3], size: f64, eye: f64) -> Box<Body> {
    let mut b = Box::new(Body::new(pos, vel, size, size, eye));
    b.server = true;
    b
}

#[derive(Debug, Clone)]
pub struct ItemEnt {
    pub id: i32,
    pub stack: Stack,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub age: i32,
    pub pickup_delay: i32,
    pub ticks: i32,
    /// Changed block during this tick (merge every 2 ticks, otherwise 40).
    pub moved: bool,
    /// Network tracking (S4: official server rules, interval 20).
    track: crate::mobs::track::Tracker,
    removed: bool,
    /// Health points (`Health`, 5 at the start): `/damage` removes them.
    pub health: i32,
    /// Physics shared with the client (`rc_physics::body`).
    pub body: Box<Body>,
    /// W8c: thrower (`Owner`: the player who threw it, the allay who threw it) — triggers
    /// `thrown_item_picked_up_by_*`.
    pub thrower: Option<crate::mobs::mob::Ent>,
    /// Yaw drawn at creation (random, never changed afterwards): it rotates the velocity at the
    /// passage of an End portal.
    pub yaw: f32,
}

impl ItemEnt {
    /// Network tracking restarted from zero (item re-announced elsewhere, S6).
    pub(crate) fn retrack(&mut self) {
        self.track = Default::default();
    }
}

#[derive(Debug, Clone)]
pub struct Orb {
    pub id: i32,
    pub value: i32,
    pub count: i32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub age: i32,
    pub ticks: i32,
    /// Network tracking (S4: official server rules, interval 20).
    track: crate::mobs::track::Tracker,
    removed: bool,
    /// Health points (`Health`, 5 at the start): `/damage` removes them.
    pub health: i32,
    /// Physics shared with the client (`rc_physics::body`).
    pub body: Box<Body>,
}

/// Split of a reward into orbs (values of the game).
pub fn orb_value(n: i32) -> i32 {
    for v in [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3] {
        if n >= v {
            return v;
        }
    }
    1
}

/// Experience dropped by a block broken with the right tool: ores (drawn range), sculk blocks
/// (constant, no draw), all cancelled by Silk Touch; mob spawner:
/// 15 + n(15) + n(15), Silk Touch included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockXp {
    Range(i32, i32),
    Const(i32),
    Spawner,
}

pub(crate) fn block_xp(name: &str) -> Option<BlockXp> {
    Some(match name.trim_start_matches("minecraft:") {
        "coal_ore" | "deepslate_coal_ore" => BlockXp::Range(0, 2),
        "nether_gold_ore" => BlockXp::Range(0, 1),
        "lapis_ore" | "deepslate_lapis_ore" | "nether_quartz_ore" => BlockXp::Range(2, 5),
        "diamond_ore" | "deepslate_diamond_ore" | "emerald_ore" | "deepslate_emerald_ore" => BlockXp::Range(3, 7),
        "redstone_ore" | "deepslate_redstone_ore" => BlockXp::Range(1, 5),
        "sculk" => BlockXp::Const(1),
        "sculk_sensor" | "calibrated_sculk_sensor" | "sculk_shrieker" | "sculk_catalyst" => BlockXp::Const(5),
        "spawner" => BlockXp::Spawner,
        _ => return None,
    })
}

fn bbox(pos: [f64; 3], size: f64) -> Aabb {
    let h = size / 2.0;
    Aabb { min: [pos[0] - h, pos[1], pos[2] - h], max: [pos[0] + h, pos[1] + size, pos[2] + h] }
}

impl Server {
    fn entity_kind(&self, name: &str) -> i32 {
        self.data
            .registry("minecraft:entity_type")
            .and_then(|r| r.entries.iter().position(|e| e == name))
            .map_or(0, |i| i as i32)
    }

    fn new_entity_id(&mut self) -> i32 {
        self.next_entity += 1;
        self.next_entity
    }

    /// Item entity created and announced to the client.
    pub(crate) fn spawn_item(&mut self, pos: [f64; 3], vel: [f64; 3], stack: Stack, pickup_delay: i32) {
        if stack.count <= 0 {
            return;
        }
        if crate::life::s2log() {
            eprintln!("singleplayer: ground item {} ×{} at {pos:?} (delay {pickup_delay})", stack.id, stack.count);
        }
        let id = self.new_entity_id();
        let yaw = self.life.rng.next_float() * 360.0;
        let add = AddEntity {
            id,
            uuid: (self.life.rng.next_long() as u128) << 64 | self.life.rng.next_long() as u64 as u128,
            kind: self.entity_kind("minecraft:item"),
            pos,
            velocity: vel,
            pitch: 0.0,
            yaw,
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let net = self.net(Some(&stack));
        let _ = self.send(Event::Entity(EntityEvent::Data { id, values: vec![(8, Meta::Item(net))] }));
        self.life.drops.push(ItemEnt {
            id,
            stack,
            pos,
            vel,
            on_ground: false,
            age: 0,
            pickup_delay,
            ticks: 0,
            moved: false,
            track: Default::default(),
            removed: false,
            health: 5,
            body: server_body(pos, vel, ITEM_SIZE, 0.2125),
            thrower: None,
            yaw,
        });
    }

    /// W8c: thrower of the last item placed on the ground.
    pub(crate) fn set_last_thrower(&mut self, who: crate::mobs::mob::Ent) {
        if let Some(d) = self.life.drops.last_mut() {
            d.thrower = Some(who);
        }
    }

    /// W8c: a mob picks up an item: thrown by the player →
    /// `thrown_item_picked_up_by_entity` (whole stack from before the pickup).
    pub(crate) fn thrown_pickup_by_mob(&mut self, item: i32, m: &crate::mobs::mob::Mob) {
        let Some(d) = self.life.drops.iter().find(|d| d.id == item && !d.removed) else { return };
        if d.thrower == Some(crate::mobs::mob::Ent::Player) {
            let s = d.stack.clone();
            self.trig_picked_up(
                "minecraft:thrown_item_picked_up_by_entity",
                &s,
                Some(crate::progress::pred::Who::Mob(m)),
            );
        }
    }

    /// Random initial velocity.
    pub(crate) fn spawn_item_random(&mut self, pos: [f64; 3], stack: Stack, delay: i32) {
        let vx = self.life.rng.next_double() * 0.2 - 0.1;
        let vz = self.life.rng.next_double() * 0.2 - 0.1;
        self.spawn_item(pos, [vx, 0.2, vz], stack, delay);
    }

    /// Loot placed at the center of a block, if the `block_drops` rule allows it.
    pub(crate) fn pop_resource(&mut self, p: [i32; 3], stack: Stack) {
        if !self.rule("minecraft:block_drops", true) {
            return;
        }
        let r = &mut self.life.rng;
        let mut off = || r.next_double() * 0.5 - 0.25;
        let x = f64::from(p[0]) + 0.5 + off();
        let y = f64::from(p[1]) + 0.5 + off() - ITEM_SIZE / 2.0;
        let z = f64::from(p[2]) + 0.5 + off();
        self.spawn_item_random([x, y, z], stack, 10);
    }

    /// Contents of a scattered container.
    pub(crate) fn drop_contents(&mut self, at: [f64; 3], mut stack: Stack) {
        let range = 1.0 - ITEM_SIZE;
        let x = at[0].floor() + self.life.rng.next_double() * range + ITEM_SIZE / 2.0;
        let y = at[1].floor() + self.life.rng.next_double() * range;
        let z = at[2].floor() + self.life.rng.next_double() * range + ITEM_SIZE / 2.0;
        while stack.count > 0 {
            let n = (self.life.rng.next_int_bound(21) + 10).min(stack.count);
            let part = Stack { count: n, ..stack.clone() };
            stack.count -= n;
            let s = 0.114_850_001_711_398_36;
            let v = [self.life.rng.triangle(0.0, s), self.life.rng.triangle(0.2, s), self.life.rng.triangle(0.0, s)];
            self.spawn_item([x, y, z], v, part, 0);
        }
    }

    pub(crate) fn spawn_item_at(&mut self, at: [f64; 3], stack: Stack) {
        self.drop_contents(at, stack);
    }

    /// Item thrown by the player: in front of them, or at random (death).
    pub(crate) fn drop_from_player_with(&mut self, stack: Stack, randomly: bool) {
        let eye = self.player_eye_h();
        let pos = [self.pos[0], self.pos[1] + eye - 0.3, self.pos[2]];
        let r = &mut self.life.rng;
        let vel = if randomly {
            let pow = r.next_float() * 0.5;
            let dir = r.next_float() * std::f32::consts::TAU;
            [f64::from(-dir.sin() * pow), 0.2, f64::from(dir.cos() * pow)]
        } else {
            let (sx, cx) = (self.pitch.to_radians().sin(), self.pitch.to_radians().cos());
            let (sy, cy) = (self.yaw.to_radians().sin(), self.yaw.to_radians().cos());
            let dir = r.next_float() * std::f32::consts::TAU;
            let pow2 = 0.02 * r.next_float();
            let jitter = (r.next_float() - r.next_float()) * 0.1;
            [
                f64::from(-sy * cx * 0.3) + f64::from(dir).cos() * f64::from(pow2),
                f64::from(-sx * 0.3 + 0.1 + jitter),
                f64::from(cy * cx * 0.3) + f64::from(dir).sin() * f64::from(pow2),
            ]
        };
        self.spawn_item(pos, vel, stack, 40);
        if !randomly {
            self.set_last_thrower(crate::mobs::mob::Ent::Player); // W8c: thrown by hand
        }
    }

    pub(crate) fn drop_from_player(&mut self, stack: Stack, _thrown: bool) {
        self.drop_from_player_with(stack, false);
    }

    /// Experience orbs.
    pub(crate) fn spawn_orbs(&mut self, pos: [f64; 3], mut amount: i32) {
        while amount > 0 {
            let v = orb_value(amount);
            amount -= v;
            let r = self.life.rng.next_int_bound(40);
            if let Some(o) = self.life.orbs.iter_mut().find(|o| {
                !o.removed && o.value == v && (o.id - r) % 40 == 0 && (0..3).all(|a| (o.pos[a] - pos[a]).abs() <= 0.5)
            }) {
                o.count += 1;
                o.age = 0;
                continue;
            }
            self.add_orb(pos, v);
        }
    }

    /// A single orb of this value, no split or merge (reward of a trade).
    pub(crate) fn add_orb(&mut self, pos: [f64; 3], v: i32) {
        self.add_orb_dir(pos, [0.0; 3], v);
    }

    /// Orb thrown roughly towards `dir` (S8: the experience orb):
    /// random velocity flipped if it opposes `dir`, start offset by half a size.
    pub(crate) fn add_orb_dir(&mut self, pos: [f64; 3], dir: [f64; 3], v: i32) {
        let id = self.new_entity_id();
        let rr = &mut self.life.rng;
        let mut vel =
            [(rr.next_double() * 0.2 - 0.1) * 2.0, rr.next_double() * 0.2 * 2.0, (rr.next_double() * 0.2 - 0.1) * 2.0];
        let l2 = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];
        let mut pos = pos;
        if l2 > 0.0 {
            if dir[0] * vel[0] + dir[1] * vel[1] + dir[2] * vel[2] < 0.0 {
                vel = vel.map(|c| -c);
            }
            let l = l2.sqrt();
            pos = std::array::from_fn(|i| pos[i] + dir[i] / l * ORB_SIZE * 0.5);
        }
        let add = AddEntity {
            id,
            uuid: (rr.next_long() as u128) << 64 | rr.next_long() as u64 as u128,
            kind: self.entity_kind("minecraft:experience_orb"),
            pos,
            velocity: vel,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: v,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let _ = self.send(Event::Entity(EntityEvent::Data { id, values: vec![(8, Meta::Int(v))] }));
        self.life.orbs.push(Orb {
            id,
            value: v,
            count: 1,
            pos,
            vel,
            on_ground: false,
            age: 0,
            ticks: 0,
            track: Default::default(),
            removed: false,
            health: 5,
            body: server_body(pos, vel, ORB_SIZE, 0.425),
        });
    }

    /// Orb summoned (`/summon`): value, count, age, health points and velocity from the NBT.
    pub(crate) fn summon_orb(&mut self, pos: [f64; 3], nbt: Option<&rc_proto::nbt::Nbt>) -> i32 {
        use crate::nbtio::int;
        let get = |k: &str, d: i64| nbt.and_then(|n| int(n, k)).unwrap_or(d) as i32;
        let (value, count, age, health) = (get("Value", 0), get("Count", 1).max(1), get("Age", 0), get("Health", 5));
        let vel = nbt.and_then(|n| crate::nbtio::motion(n.get("Motion"))).unwrap_or_default();
        let id = self.new_entity_id();
        let rr = &mut self.life.rng;
        let add = AddEntity {
            id,
            uuid: (rr.next_long() as u128) << 64 | rr.next_long() as u64 as u128,
            kind: self.entity_kind("minecraft:experience_orb"),
            pos,
            velocity: vel,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: value,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        let _ = self.send(Event::Entity(EntityEvent::Data { id, values: vec![(8, Meta::Int(value))] }));
        self.life.orbs.push(Orb {
            id,
            value,
            count,
            pos,
            vel,
            on_ground: false,
            age,
            ticks: 0,
            track: Default::default(),
            removed: false,
            health,
            body: server_body(pos, vel, ORB_SIZE, 0.425),
        });
        id
    }

    /// Players an orb can follow (the local player).
    fn orb_followers(&self) -> [Follow; 1] {
        [Follow {
            id: self.player_id,
            pos: self.pos,
            eye: self.player_eye_h(),
            spectator: self.game_mode == 3 || !self.player_ready(),
            dead: self.life.vit.dead,
        }]
    }

    /// Tick of items and orbs: physics, merge, pickup, disappearance.
    pub(crate) fn drops_tick(&mut self) {
        let mut drops = std::mem::take(&mut self.life.drops);
        // Network tracking BEFORE the entity tick, like the official server (the client receives
        // the state of the previous tick then does the current tick itself).
        for d in drops.iter_mut().filter(|d| !d.removed) {
            self.track_object(&mut d.track, d.id, d.pos, d.vel, d.on_ground, 20);
        }
        for d in drops.iter_mut().filter(|d| !d.removed) {
            d.ticks += 1;
            if d.pickup_delay > 0 && d.pickup_delay != 32767 {
                d.pickup_delay -= 1;
            }
            let old = d.pos;
            // Same physics as the client (`rc_physics::body`): single-precision constants,
            // braking block, water and lava, currents, block effects, block push-out.
            let (mv, wet, below, support) = {
                let w = self.world();
                let b = &mut d.body;
                // Water state at the start of the tick (the one the movement reads).
                let wet = b.was_in_water();
                (b.pos, b.vel, b.on_ground) = (d.pos, d.vel, d.on_ground);
                b.fast_lava = self.fast_lava();
                let mut r = crate::mobs::jrand::JRand::new(i64::from(d.id) << 32 ^ i64::from(d.ticks));
                b.tick_item_server(&w, &self.phys, d.ticks, d.id, &mut || r.next_float());
                (d.pos, d.vel, d.on_ground) = (b.pos, b.vel, b.on_ground);
                (b.moved, wet, b.below(&self.phys, &w, 0.2), b.support())
            };
            // Fall distance then landing (`hit_ground` event, block effects).
            if self.fall_step(&mut d.body.fall, old, mv, wet, d.on_ground) {
                let id = &d.stack.id;
                let dampens = id.ends_with("_wool")
                    || (id.ends_with("_carpet") && !id.contains("moss"))
                    || id.ends_with("_wool_slab")
                    || id.ends_with("_wool_stairs");
                self.landing_effects(d.pos, below, support, Src::Item { owner: None, dampens });
            }
            if d.on_ground {
                d.body.fall = 0.0;
            }
            // Blocks passed through (or, at rest, those it rests in): a hopper takes it at once.
            self.hopper_item_inside(old, d);
            if d.removed {
                continue;
            }
            if d.age != -32768 {
                d.age += 1;
            }
            if d.age >= LIFETIME || d.pos[1] < f64::from(self.dim.height.min_y) - 64.0 {
                d.removed = true;
            }
            let moved = (0..3).any(|a| old[a].floor() != d.pos[a].floor());
            d.moved = moved;
        }
        // Merge (every 2 ticks while moving, 40 at rest).
        let n = drops.len();
        for i in 0..n {
            let rate = if drops[i].moved { 2 } else { 40 };
            if drops[i].removed || drops[i].ticks % rate != 0 {
                continue;
            }
            for j in 0..n {
                if i == j || drops[j].removed || drops[i].removed {
                    continue;
                }
                let (a, b) = (&drops[i], &drops[j]);
                let max = self.life.items.max_stack(&a.stack);
                // Neither pickup blocked (32767) nor eternal item (age −32768): never merged.
                let mergeable =
                    |e: &ItemEnt| e.pickup_delay != 32767 && e.age != -32768 && e.age < LIFETIME && e.stack.count < max;
                let near = (a.pos[0] - b.pos[0]).abs() <= ITEM_SIZE + 0.5
                    && (a.pos[2] - b.pos[2]).abs() <= ITEM_SIZE + 0.5
                    && (a.pos[1] - b.pos[1]).abs() <= ITEM_SIZE;
                if !near
                    || !mergeable(a)
                    || !mergeable(b)
                    || !same(&a.stack, &b.stack)
                    || a.stack.count + b.stack.count > max
                {
                    continue;
                }
                // The biggest stack absorbs the smallest.
                let (to, from) = if b.stack.count < a.stack.count { (i, j) } else { (j, i) };
                let moved = drops[from].stack.count.min(max.min(64) - drops[to].stack.count);
                drops[to].stack.count += moved;
                drops[from].stack.count -= moved;
                drops[to].pickup_delay = drops[to].pickup_delay.max(drops[from].pickup_delay);
                drops[to].age = drops[to].age.min(drops[from].age);
                let net = self.net(Some(&drops[to].stack));
                let _ = self
                    .send(Event::Entity(EntityEvent::Data { id: drops[to].id, values: vec![(8, Meta::Item(net))] }));
                if drops[from].stack.count <= 0 {
                    drops[from].removed = true;
                }
            }
        }
        // Pickup: player box inflated by 1 / 0.5 / 1.
        let alive = !self.life.vit.dead && self.game_mode != 3 && self.player_ready();
        if alive {
            let h = if self.sneaking { 1.5 } else { 1.8 };
            let reach = Aabb {
                min: [self.pos[0] - 1.3, self.pos[1] - 0.5, self.pos[2] - 1.3],
                max: [self.pos[0] + 1.3, self.pos[1] + h + 0.5, self.pos[2] + 1.3],
            };
            for d in drops.iter_mut().filter(|d| !d.removed && d.pickup_delay == 0) {
                let b = bbox(d.pos, ITEM_SIZE);
                if !(0..3).all(|a| b.max[a] > reach.min[a] && b.min[a] < reach.max[a]) {
                    continue;
                }
                let before = d.stack.count;
                let creative = self.game_mode == 1;
                let selected = self.held as usize;
                let rest = self.pickup_into_inventory(d.stack.clone(), creative, selected);
                if crate::life::s2log() {
                    let where_: Vec<(usize, String)> =
                        self.slots.iter().enumerate().filter_map(|(i, s)| Some((i, s.as_ref()?.id.clone()))).collect();
                    eprintln!("singleplayer: pickup {} (held {selected}) → {where_:?}", d.stack.id);
                }
                let after = rest.as_ref().map_or(0, |s| s.count);
                if after < before {
                    self.s9_picked_up(&d.stack.id, before); // S9
                    // W8c: the reaction to the pickup of a player item: stack seen whole if everything is taken, otherwise
                    // what remains; player thrower → `…_by_entity` (the player), any thrower →
                    // `…_by_player`.
                    let seen = rest.clone().unwrap_or_else(|| d.stack.clone());
                    if let Some(t) = d.thrower {
                        use crate::progress::pred::Who;
                        let me = Who::E(crate::commands::ents::E::Player);
                        if t == crate::mobs::mob::Ent::Player {
                            self.trig_picked_up("minecraft:thrown_item_picked_up_by_entity", &seen, Some(me));
                        }
                        let mob = match t {
                            crate::mobs::mob::Ent::Mob(id) => self.mob_ref(id).map(|m| (*m).clone()),
                            crate::mobs::mob::Ent::Player => None,
                        };
                        let who = match &mob {
                            Some(m) => Some(Who::Mob(m)),
                            None if t == crate::mobs::mob::Ent::Player => Some(me),
                            None => None,
                        };
                        if who.is_some() {
                            self.trig_picked_up("minecraft:thrown_item_picked_up_by_player", &seen, who);
                        }
                    }
                    let _ = self.send(Event::Misc(rc_client::misc::Misc::Take {
                        item: d.id,
                        collector: self.player_id,
                        amount: before,
                    }));
                    match rest {
                        None => d.removed = true,
                        Some(r) => {
                            d.stack = r;
                            let net = self.net(Some(&d.stack));
                            let _ = self.send(Event::Entity(EntityEvent::Data {
                                id: d.id,
                                values: vec![(8, Meta::Item(net))],
                            }));
                        }
                    }
                }
            }
        }
        let gone: Vec<i32> = drops.iter().filter(|d| d.removed).map(|d| d.id).collect();
        drops.retain(|d| !d.removed);
        self.life.drops = drops;
        let mut orbs = std::mem::take(&mut self.life.orbs);
        self.orbs_tick(&mut orbs);
        let gone_orbs: Vec<i32> = orbs.iter().filter(|o| o.removed).map(|o| o.id).collect();
        orbs.retain(|o| !o.removed);
        self.life.orbs = orbs;
        let all: Vec<i32> = gone.into_iter().chain(gone_orbs).collect();
        if !all.is_empty() {
            // Scoreboard: items and orbs removed (command UUID derived from the id).
            if self.cmd.scores.has_entries() {
                for &i in &all {
                    self.entity_scores_removed(self.ent_uuid(crate::commands::ents::E::Item(i)));
                }
            }
            let _ = self.send(Event::Entity(EntityEvent::Remove(all)));
        }
    }

    /// Storing a picked-up item: returns what did not fit.
    pub(crate) fn pickup_into_inventory(&mut self, s: Stack, creative: bool, selected: usize) -> Option<Stack> {
        let items = &self.life.items;
        let count = s.count;
        // Adding an item to the inventory completes as best it can; the rest stays on the ground.
        let mut probe = self.slots.clone();
        match inventory_add(&mut probe, items, selected, s.clone(), creative) {
            Some(rest) if rest.count == count => Some(rest),
            _ => {
                // Starts again counting what was placed.
                let before: i32 = self.slots.iter().flatten().filter(|t| same(t, &s)).map(|t| t.count).sum();
                let _ = inventory_add(&mut self.slots, items, selected, s.clone(), creative);
                let after: i32 = self.slots.iter().flatten().filter(|t| same(t, &s)).map(|t| t.count).sum();
                let placed = if creative { count } else { after - before };
                (placed < count).then(|| Stack { count: count - placed, ..s })
            }
        }
    }

    fn orbs_tick(&mut self, orbs: &mut [Orb]) {
        let follow = !self.life.vit.dead && self.game_mode != 3;
        let players = self.orb_followers();
        let fast = self.fast_lava();
        // Network tracking before the tick (order of the official server).
        for o in orbs.iter_mut().filter(|o| !o.removed) {
            self.track_object(&mut o.track, o.id, o.pos, o.vel, o.on_ground, 20);
        }
        for o in orbs.iter_mut().filter(|o| !o.removed) {
            o.ticks += 1;
            // Same physics as the client (`rc_physics::body`), no skipped tick.
            let w = self.world();
            let from = o.pos;
            let b = &mut o.body;
            let wet = b.was_in_water();
            (b.pos, b.vel, b.on_ground) = (o.pos, o.vel, o.on_ground);
            b.fast_lava = fast;
            let mut r = crate::mobs::jrand::JRand::new(i64::from(o.id) << 32 ^ i64::from(o.ticks));
            b.tick_orb_server(&w, &self.phys, &players, &mut || r.next_float());
            (o.pos, o.vel, o.on_ground) = (b.pos, b.vel, b.on_ground);
            let (mv, below, support) = (b.moved, b.below(&self.phys, &w, 0.2), b.support());
            if self.fall_step(&mut o.body.fall, from, mv, wet, o.on_ground) {
                self.landing_effects(o.pos, below, support, Src::Other);
            }
            if o.on_ground {
                o.body.fall = 0.0;
            }
            o.age += 1;
            if o.age >= LIFETIME {
                o.removed = true;
            }
        }
        // Pickup: one orb every 2 ticks.
        if follow && self.player_ready() {
            if self.life.vit.xp_delay > 0 {
                self.life.vit.xp_delay -= 1;
            }
            let h = if self.sneaking { 1.5 } else { 1.8 };
            let touching: Vec<usize> = orbs
                .iter()
                .enumerate()
                .filter(|(_, o)| !o.removed)
                .filter(|(_, o)| {
                    let b = bbox(o.pos, ORB_SIZE);
                    b.max[0] > self.pos[0] - 1.3
                        && b.min[0] < self.pos[0] + 1.3
                        && b.max[1] > self.pos[1] - 0.5
                        && b.min[1] < self.pos[1] + h + 0.5
                        && b.max[2] > self.pos[2] - 1.3
                        && b.min[2] < self.pos[2] + 1.3
                })
                .map(|(i, _)| i)
                .collect();
            if !touching.is_empty() && self.life.vit.xp_delay == 0 {
                let i = touching[self.life.rng.next_int_bound(touching.len() as i32) as usize];
                self.life.vit.xp_delay = 2;
                let _ = self.send(Event::Misc(rc_client::misc::Misc::Take {
                    item: orbs[i].id,
                    collector: self.player_id,
                    amount: 1,
                }));
                let v = orbs[i].value;
                self.give_xp(v);
                orbs[i].count -= 1;
                if orbs[i].count <= 0 {
                    orbs[i].removed = true;
                }
            }
        }
    }

    /// Ground items read with a chunk: made live (pickable, simulated).
    pub(crate) fn adopt_items(&mut self, loaded: &[(i32, i32)]) {
        use rc_proto::nbt::Nbt;
        for p in loaded {
            let Some(list) = self.entities.get_mut(p) else { continue };
            type Ents = Vec<(i32, Nbt)>;
            let (items, rest): (Ents, Ents) = std::mem::take(list)
                .into_iter()
                .partition(|(_, e)| e.get("id").and_then(Nbt::as_str) == Some("minecraft:item"));
            *list = rest;
            if items.is_empty() {
                continue;
            }
            self.life.drop_chunks.insert(*p);
            for (_, e) in items {
                let Some(stack) = e.get("Item").and_then(Stack::from_nbt) else { continue };
                let pos = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
                let vel = crate::nbtio::motion(e.get("Motion")).unwrap_or_default();
                let delay = crate::nbtio::int(&e, "PickupDelay").unwrap_or(0) as i32;
                let age = crate::nbtio::int(&e, "Age").unwrap_or(0) as i32;
                let health = crate::nbtio::int(&e, "Health").unwrap_or(5) as i32;
                // W8c: thrower re-read (only the player is recognized).
                let thrower = crate::nbtio::uuid(e.get("Thrower")) == Some(self.uuid);
                self.spawn_item(pos, vel, stack, delay);
                if let Some(d) = self.life.drops.last_mut() {
                    d.age = age;
                    d.health = health;
                    if let Some([yaw, _]) = crate::nbtio::f32s::<2>(e.get("Rotation")) {
                        d.yaw = yaw;
                    }
                    if thrower {
                        d.thrower = Some(crate::mobs::mob::Ent::Player);
                    }
                }
            }
        }
    }

    /// Entity chunks to rewrite (`entities/`): those that carry ground items, or
    /// carried them at the previous save; entities read kept as they are.
    pub(crate) fn drops_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        use rc_proto::nbt::Nbt;
        use std::collections::BTreeMap;
        let mut by_chunk: BTreeMap<(i32, i32), Vec<Nbt>> = BTreeMap::new();
        for d in &self.life.drops {
            let c = ((d.pos[0].floor() as i32) >> 4, (d.pos[2].floor() as i32) >> 4);
            let dbl = |v: [f64; 3]| Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect());
            let uuid = [d.id, 0x5332_0000, 0, d.id.wrapping_mul(31)];
            let thrower = (d.thrower == Some(crate::mobs::mob::Ent::Player))
                .then(|| ("Thrower".to_owned(), crate::nbtio::uuid_nbt(self.uuid))); // W8c
            let fields = [
                ("id".into(), Nbt::String("minecraft:item".into())),
                ("Pos".into(), dbl(d.pos)),
                ("Motion".into(), dbl(d.vel)),
                ("Rotation".into(), Nbt::List(vec![Nbt::Float(d.yaw), Nbt::Float(0.0)])),
                ("OnGround".into(), Nbt::Byte(i8::from(d.on_ground))),
                ("Health".into(), Nbt::Short(d.health as i16)),
                ("Age".into(), Nbt::Short(d.age as i16)),
                ("PickupDelay".into(), Nbt::Short(d.pickup_delay as i16)),
                ("UUID".into(), Nbt::IntArray(uuid.to_vec())),
                ("Item".into(), d.stack.to_nbt(None)),
            ];
            by_chunk.entry(c).or_default().push(Nbt::Compound(thrower.into_iter().chain(fields).collect()));
        }
        self.mobs_into_chunks(&mut by_chunk); // S4: mobs
        self.projectiles_into_chunks(&mut by_chunk); // S8: projectiles, clouds
        self.carts_into_chunks(&mut by_chunk); // G5-F06: minecarts
        let now: std::collections::BTreeSet<(i32, i32)> = by_chunk.keys().copied().collect();
        let mut all = now.clone();
        all.extend(self.life.drop_chunks.iter().copied());
        for c in all {
            let mut ents: Vec<Nbt> =
                self.entities.get(&c).map(|l| l.iter().map(|e| e.1.clone()).collect()).unwrap_or_default();
            ents.extend(by_chunk.remove(&c).unwrap_or_default());
            let root = Nbt::Compound(vec![
                ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
                ("Position".into(), Nbt::IntArray(vec![c.0, c.1])),
                ("Entities".into(), Nbt::List(ents)),
            ]);
            let path = self.dim.dir.join("entities").join(crate::region::region_name(c.0, c.1));
            jobs.push(crate::save::Job::Chunk { path, cx: c.0, cz: c.1, nbt: crate::nbtio::encode_root(&root) });
        }
        self.life.drop_chunks = now;
    }

    /// Metadata and equipment of an entity read from the world (`entities.md` § 3): common
    /// fields, mob (no AI, left-handed), age, ground item, worn equipment.
    pub(crate) fn send_entity_meta(&self, id: i32, e: &rc_proto::nbt::Nbt) {
        use crate::nbtio::int;
        use rc_proto::nbt::Nbt;
        let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
        let mut values: Vec<(u8, Meta)> = Vec::new();
        let mut shared = 0i8;
        if int(e, "Fire").is_some_and(|f| f > 0) {
            shared |= 1;
        }
        if flag("Invisible") {
            shared |= 0x20;
        }
        if flag("Glowing") {
            shared |= 0x40;
        }
        if shared != 0 {
            values.push((0, Meta::Byte(shared)));
        }
        if let Some(a) = int(e, "Air").filter(|&a| a != 300) {
            values.push((1, Meta::Int(a as i32)));
        }
        if let Some(n) = e.get("CustomName") {
            values.push((2, Meta::Text(Some(n.clone()))));
        }
        if flag("CustomNameVisible") {
            values.push((3, Meta::Bool(true)));
        }
        if flag("Silent") {
            values.push((4, Meta::Bool(true)));
        }
        if flag("NoGravity") {
            values.push((5, Meta::Bool(true)));
        }
        let kind = e.get("id").and_then(Nbt::as_str).unwrap_or("");
        if kind == "minecraft:item" {
            if let Some(s) = e.get("Item").and_then(Stack::from_nbt) {
                values.push((8, Meta::Item(self.net(Some(&s)))));
            }
        } else if e.get("Health").is_some() {
            // Living mob: health, then `Mob` flags and age of the "growing mob" layer.
            if let Some(h) = crate::nbtio::float(e, "Health") {
                values.push((9, Meta::Float(h)));
            }
            if e.get("LeftHanded").is_some() || e.get("PersistenceRequired").is_some() {
                let mut m = 0i8;
                if flag("NoAI") {
                    m |= 1;
                }
                if flag("LeftHanded") {
                    m |= 2;
                }
                if m != 0 {
                    values.push((15, Meta::Byte(m)));
                }
                if let Some(age) = int(e, "Age")
                    && age < 0
                {
                    values.push((16, Meta::Bool(true)));
                }
                if flag("AgeLocked") {
                    values.push((17, Meta::Bool(true)));
                }
            }
        }
        self.deco_meta(e, &mut values); // paintings, frames, stands, cushions, mannequins
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id, values }));
        }
        // Equipment (`equipment`: hand, armor).
        if let Some(Nbt::Compound(eq)) = e.get("equipment") {
            let slot_of = |k: &str| match k {
                "mainhand" => Some(0u8),
                "offhand" => Some(1),
                "feet" => Some(2),
                "legs" => Some(3),
                "chest" => Some(4),
                "head" => Some(5),
                "body" => Some(6),
                "saddle" => Some(7),
                _ => None,
            };
            let slots: rc_proto_v777::entity::Equipment =
                eq.iter().filter_map(|(k, v)| Some((slot_of(k)?, self.net(Some(&Stack::from_nbt(v)?))))).collect();
            if !slots.is_empty() {
                let _ = self.send(Event::Entity(EntityEvent::Equipment { id, slots }));
            }
        }
    }

    /// Block broken by the player, just before its removal: contents thrown,
    /// tool wear, exhaustion, loot and experience (survival only).
    pub(crate) fn on_block_broken(&mut self, p: [i32; 3], state: u32) {
        self.piglin_block_destroyed(p); // F13: block guarded by the piglins
        self.container_broken(p);
        if self.game_mode == 1 {
            return;
        }
        let Some(block) = self.data.block_of(state).map(|b| b.name.clone()) else { return };
        let held = 36 + self.held as usize;
        let tool = self.slots[held].clone();
        // Wear: non-zero hardness.
        let hardness = self.data.hardness.get(state as usize).copied().unwrap_or(0.0);
        if hardness != 0.0
            && let Some(t) = &tool
            && let Some(info) = self.life.items.id(&t.id).and_then(|i| self.data.items.get(i as usize))
            && let Some(tc) = &info.tool
        {
            let n = tc.damage_per_block as i32;
            if n > 0 {
                self.hurt_item(held, n);
            }
        }
        self.s9_tool_used(tool.as_ref()); // S9: tool wear after mining
        // Right tool (or block with no requirement): loot, exhaustion, experience.
        let needs = self.data.needs_tool.get(state as usize).copied().unwrap_or(false);
        let correct = !needs || self.tool_correct(tool.as_ref(), state);
        if !correct {
            return;
        }
        self.s9_block_mined(&block); // S9: destruction by a player
        self.life.vit.exhaust(0.005, self.abilities().invulnerable);
        let props: Vec<(String, String)> =
            self.data.state_properties(state).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let be = self.block_entity(p).cloned();
        let path = block.trim_start_matches("minecraft:");
        let table_id = format!("minecraft:blocks/{path}");
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, name: &str| {
            let id = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let seq =
            self.life.pack.get("loot_table", &table_id).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        // W8c: generator taken out for the time of the draw; the world is lent to the conditions (location
        // offset of tall grass, breaking player).
        let mut rng = match &seq {
            Some(s) => *self.life.seqs.get(s),
            None => self.life.rng,
        };
        let drops = {
            let world = self.loot_world(crate::progress::pred::Ctx {
                this: Some(crate::progress::pred::Who::E(crate::commands::ents::E::Player)),
                origin: Some([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]),
                block: Some(state),
                tool: Some(tool.as_ref()),
                ..Default::default()
            });
            let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
            let ctx = crate::loot::Ctx {
                block: Some((block.as_str(), props)),
                tool: Some(tool.as_ref()),
                explosion: None,
                block_entity: be.as_ref(),
                luck: 0.0,
                world: Some(&world),
                ..Default::default()
            };
            loot.drops(&table_id, &ctx, &mut rng)
        };
        match &seq {
            Some(s) => *self.life.seqs.get(s) = rng,
            None => self.life.rng = rng,
        }
        for d in drops {
            self.pop_resource(p, d);
        }
        self.drop_block_xp(p, &block, tool.as_ref());
        let silk = tool.as_ref().is_some_and(|t| self.life.items.enchantment(t, "minecraft:silk_touch") > 0);
        self.infested_broken(p, &block, tool.as_ref()); // F01: silverfish of infested blocks
        self.hive_broken(p, &block, tool.as_ref()); // F11: bees released, anger
        // W8c: `bee_nest_destroyed`: bees left inside (Silk Touch: all).
        if block == "minecraft:beehive" || block == "minecraft:bee_nest" {
            let bees = if silk { crate::nbtio::list(be.as_ref().and_then(|b| b.get("bees"))).len() as i32 } else { 0 };
            self.trig_bee_nest(state, tool.as_ref(), bees);
        }
    }

    /// Experience of a broken block: draw, Silk Touch that cancels it
    /// (except for the spawner), then the loot rule.
    pub(crate) fn drop_block_xp(&mut self, p: [i32; 3], block: &str, tool: Option<&Stack>) {
        let silk = tool.is_some_and(|t| self.life.items.enchantment(t, "minecraft:silk_touch") > 0);
        let xp = match block_xp(block) {
            Some(BlockXp::Range(a, b)) => {
                let n = self.life.rng.int_between(a, b);
                if silk { 0 } else { n }
            }
            Some(BlockXp::Const(n)) => {
                if silk {
                    0
                } else {
                    n
                }
            }
            Some(BlockXp::Spawner) => {
                let r = &mut self.life.rng;
                15 + r.next_int_bound(15) + r.next_int_bound(15)
            }
            None => 0,
        };
        if xp > 0 && self.rule("minecraft:block_drops", true) {
            self.spawn_orbs([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5], xp);
        }
    }

    pub(crate) fn tool_correct(&self, tool: Option<&Stack>, state: u32) -> bool {
        let block = self.data.state(state).map_or(u32::MAX, |s| u32::from(s.block));
        let Some(t) = tool
            .and_then(|s| self.life.items.id(&s.id))
            .and_then(|i| self.data.items.get(i as usize))
            .and_then(|i| i.tool.as_ref())
        else {
            return false;
        };
        t.rules
            .iter()
            .find(|r| r.correct.is_some() && self.tag_contains(&r.blocks, block))
            .and_then(|r| r.correct)
            .unwrap_or(false)
    }

    fn tag_contains(&self, set: &rc_data::BlockSet, block: u32) -> bool {
        match set {
            rc_data::BlockSet::Tag(t) => self.block_tags.get(t).is_some_and(|v| v.binary_search(&block).is_ok()),
            rc_data::BlockSet::Blocks(b) => b.contains(&block),
        }
    }

    /// Wear of an item of menu 0: Unbreaking, breaking.
    /// Remaining wear after Unbreaking: each point is avoided with the chance of the enchantment
    /// (armor: 2n / (5n + 5), other: n / (n + 1)), world's random draw.
    pub(crate) fn unbreaking_kept(&mut self, s: &Stack, n: i32, armor: bool) -> i32 {
        let unbreaking = self.life.items.enchantment(s, "minecraft:unbreaking");
        if unbreaking <= 0 {
            return n;
        }
        let u = unbreaking as f32;
        let p = if armor { 2.0 * u / (5.0 * u + 5.0) } else { u / (u + 1.0) };
        let mut removed = 0;
        for _ in 0..n {
            if self.life.rng.next_float() < p {
                removed += 1;
            }
        }
        n - removed
    }

    pub(crate) fn hurt_item(&mut self, slot: usize, n: i32) {
        if self.game_mode == 1 {
            return;
        }
        let Some(s) = self.slots[slot].clone() else { return };
        let items = &self.life.items;
        if !items.damageable(&s) {
            return;
        }
        let n2 = self.unbreaking_kept(&s, n, (5..9).contains(&slot));
        if n2 <= 0 {
            return;
        }
        let items = &self.life.items;
        let max = items.max_damage(&s);
        let dmg = items.damage(&s) + n2;
        self.s9_item_damaged(&s, dmg.min(max), dmg >= max); // S9
        let mut s = s;
        if dmg >= max {
            // Broken: stack −1, break sound.
            s.count -= 1;
            crate::items::remove_component(&mut s, "minecraft:damage");
            self.slots[slot] = (s.count > 0).then_some(s);
            self.use_stack_emptied(slot); // bow, brush, shield broken mid-use
            // W8c: break event specific to the slot (47 main hand, 48 off hand, 49-52
            // head, chest, legs, feet), like the equipment slot.
            let event = match slot {
                45 => 48,
                5 => 49,
                6 => 50,
                7 => 51,
                8 => 52,
                _ => 47,
            };
            let _ = self.send(Event::Entity(EntityEvent::Event { id: self.player_id, event }));
        } else {
            crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(dmg));
            self.slots[slot] = Some(s);
        }
    }
}

// ---------- F03: ground items taken by a mob (sulfur cube) ----------

impl ItemEnt {
    /// Removed from the world (taken or destroyed) but not yet forgotten.
    pub(crate) fn is_removed(&self) -> bool {
        self.removed
    }

    /// Removed from the world (poured into a hopper): disappears at the end of the item tick.
    pub(crate) fn mark_removed(&mut self) {
        self.removed = true;
    }
}

impl Orb {
    /// S8: removed (merge of thrown orbs).
    pub(crate) fn is_removed(&self) -> bool {
        self.removed
    }

    /// Push (explosion blast): velocity sent back at the next tracking, at once if
    /// the orb has just been hurt.
    pub(crate) fn push_sync(&mut self, kb: [f64; 3], hurt: bool) {
        self.vel = [self.vel[0] + kb[0], self.vel[1] + kb[1], self.vel[2] + kb[2]];
        self.track.needs_sync = true;
        self.track.sync_velocity |= hurt;
    }
}

impl ItemEnt {
    /// Push (explosion blast): as for the orb.
    pub(crate) fn push_sync(&mut self, kb: [f64; 3], hurt: bool) {
        self.vel = [self.vel[0] + kb[0], self.vel[1] + kb[1], self.vel[2] + kb[2]];
        self.track.needs_sync = true;
        self.track.sync_velocity |= hurt;
    }
}

impl Server {
    /// A mob takes one copy of ground item `item` (`split(1)` then `take`); the emptied stack
    /// disappears at the next item tick, as in the game.
    /// F05: a mob takes `n` copies (inventory pickup): a single "taken" packet, the emptied
    /// stack disappears.
    pub(crate) fn mob_take(&mut self, item: i32, collector: i32, n: i32) {
        let Some(d) = self.life.drops.iter_mut().find(|d| d.id == item && !d.removed) else { return };
        let n = n.min(d.stack.count);
        d.stack.count -= n;
        let left = d.stack.count;
        if left <= 0 {
            d.removed = true;
        }
        let _ = self.send(Event::Misc(rc_client::misc::Misc::Take { item, collector, amount: n }));
        if left > 0
            && let Some(d) = self.life.drops.iter().find(|d| d.id == item)
        {
            let net = self.net(Some(&d.stack));
            let _ = self.send(Event::Entity(EntityEvent::Data { id: item, values: vec![(8, Meta::Item(net))] }));
        }
    }

    pub(crate) fn mob_take_one(&mut self, item: i32, collector: i32) -> Option<Stack> {
        let d = self.life.drops.iter_mut().find(|d| d.id == item && !d.removed)?;
        if d.stack.count <= 0 {
            return None;
        }
        let one = Stack { count: 1, ..d.stack.clone() };
        d.stack.count -= 1;
        let left = d.stack.count;
        let _ = self.send(Event::Misc(rc_client::misc::Misc::Take { item, collector, amount: 1 }));
        if left <= 0 {
            if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == item) {
                d.removed = true;
            }
        } else if let Some(d) = self.life.drops.iter().find(|d| d.id == item) {
            let net = self.net(Some(&d.stack));
            let _ = self.send(Event::Entity(EntityEvent::Data { id: item, values: vec![(8, Meta::Item(net))] }));
        }
        Some(one)
    }
}

// ---------- Damage taken by ground items and orbs (`/damage`) ----------

impl Server {
    /// `damage_resistant` component of the stack (patch, otherwise prototype): tag of damage types.
    fn resistant_tag(&self, s: &Stack) -> Option<String> {
        if let Some(c) = crate::items::component(s, "minecraft:damage_resistant") {
            return c.get("types").and_then(rc_proto::nbt::Nbt::as_str).map(str::to_owned);
        }
        let v = self.life.items.default_component(s, "minecraft:damage_resistant")?;
        v["types"].as_str().map(str::to_owned)
    }

    /// Damage to a ground item: refused if a mob causes it without `mob_griefing` or if the
    /// stack resists it; otherwise health points removed (the game's integer truncation), item destroyed at 0
    /// with its contents (box, bag) spilled in its place. True if they landed.
    pub(crate) fn item_hurt(&mut self, id: i32, ty: &str, amount: f32, by_mob: bool) -> bool {
        let Some(k) = self.life.drops.iter().position(|d| d.id == id && !d.removed) else { return false };
        if by_mob && !self.rule("minecraft:mob_griefing", true) {
            return false;
        }
        if let Some(tag) = self.resistant_tag(&self.life.drops[k].stack)
            && self.dmg_tag(ty, tag.trim_start_matches('#'))
        {
            return false;
        }
        let d = &mut self.life.drops[k];
        d.health = (d.health as f32 - amount) as i32;
        if d.health > 0 {
            return true;
        }
        d.removed = true;
        let pos = d.pos;
        let mut spill = Vec::new();
        use rc_proto::nbt::Nbt;
        if let Some(Nbt::List(l)) = crate::items::component(&d.stack, "minecraft:container") {
            spill.extend(l.iter().filter_map(|e| e.get("item").and_then(Stack::from_nbt)));
        }
        if let Some(Nbt::List(l)) = crate::items::component(&d.stack, "minecraft:bundle_contents") {
            spill.extend(l.iter().filter_map(Stack::from_nbt));
        }
        for s in spill {
            self.spawn_item_random(pos, s, 0);
        }
        true
    }

    /// Damage to an experience orb: health points removed, orb removed at 0.
    pub(crate) fn orb_hurt(&mut self, id: i32, amount: f32) -> bool {
        let Some(o) = self.life.orbs.iter_mut().find(|o| o.id == id && !o.removed) else { return false };
        o.health = (o.health as f32 - amount) as i32;
        if o.health <= 0 {
            o.removed = true;
        }
        true
    }
}
