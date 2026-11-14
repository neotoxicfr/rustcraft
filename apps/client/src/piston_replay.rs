//! Pistons on the client side (`block-entities.md` § 1.4): the server does not send the moving
//! blocks, the client recreates them by replaying the piston's block event (scheduled by
//! `rc_sim::PistonReplay`, same resolution as the server), then their tick pushes the local
//! player and places the final block if it has not arrived from the server in the meantime.

use rc_physics::Aabb;
use rc_physics::piston::{BodyTarget, Moving, PistonTarget, Sweep, carry_stuck, push_collided};
use rc_sim::{MovingBlock, PistonWorld, ReplayOp, ReplayWorld};

use crate::app_meshing::ChangeOrigin;
use crate::block_entities::{BlockEntities, PistonTick};
use crate::{App, GridWorld, entity_motion, environ, world_sounds};

/// The client world as seen by the replay.
struct ClientWorld<'a> {
    view: environ::WorldView<'a>,
    bes: &'a BlockEntities,
    bounds: [f64; 4],
}

impl PistonWorld for ClientWorld<'_> {
    fn state(&self, p: [i32; 3]) -> u32 {
        self.view.state(p[0], p[1], p[2])
    }
    fn min_y(&self) -> i32 {
        self.view.min_y
    }
    fn max_y(&self) -> i32 {
        self.view.min_y + self.view.height - 1
    }
    fn in_border(&self, p: [i32; 3]) -> bool {
        let (x, z) = (f64::from(p[0]), f64::from(p[2]));
        x >= self.bounds[0] && x < self.bounds[1] && z >= self.bounds[2] && z < self.bounds[3]
    }
}

impl ReplayWorld for ClientWorld<'_> {
    fn mover(&self, p: [i32; 3]) -> Option<MovingBlock> {
        let m = self.bes.mover(p)?;
        Some(MovingBlock { moved: m.moved, dir: m.dir, extending: m.extending, source: m.source })
    }
}

/// One tick of a moving block's push: the area it sweeps and the honey drag above it.
#[derive(Clone, Copy)]
struct Push<'a> {
    pos: [i32; 3],
    m: &'a Moving,
    sweep: Option<&'a Sweep>,
    carry: Option<&'a (Aabb, u8, f64)>,
    /// Both areas, for the quick "is it touched at all" test.
    areas: &'a [Aabb],
    time: i64,
}

impl Push<'_> {
    /// The sweep's push, then the honey's drag, applied to one entity.
    fn apply(&self, e: &mut dyn PistonTarget) {
        if let Some(sw) = self.sweep {
            push_collided(sw, self.pos, e);
        }
        if let Some(c) = self.carry {
            carry_stuck(c, self.pos, e);
        }
    }
}

/// An entity the client shows without simulating it as a body: it keeps no velocity of its own
/// here (a projectile excepted, which moves itself on this side), no supporting block, and its
/// interpolation follows the push. The vehicle the local player pilots is simulated here: like a
/// projectile, its blocked axes lose their speed and a slime block launches it.
struct TrackedTarget<'a> {
    e: &'a mut crate::entities::Entity,
    dims: &'a [[f32; 4]],
    w: &'a dyn rc_physics::World,
    time: i64,
    piloted_here: bool,
}

impl PistonTarget for TrackedTarget<'_> {
    fn bbox(&self) -> Aabb {
        self.e.cached_box(self.dims, &|_| None, &|_| false)
    }
    fn position(&self) -> [f64; 3] {
        self.e.pos
    }
    fn grounded(&self) -> bool {
        self.e.on_ground
    }
    // No supporting block known here: only the centred test of the honey drag applies.
    fn supported_by(&self, _: [i32; 3]) -> bool {
        false
    }
    // Only what simulates its own movement on this side bounces off a slime block here.
    fn launch(&mut self, axis: usize, v: f64) {
        if self.moves_itself() {
            self.e.velocity[axis] = v;
        }
    }
    fn piston_push(&mut self, d: [f64; 3]) {
        let d = self.e.piston_deltas.limit(self.time, d);
        if d == [0.0; 3] {
            return;
        }
        let start = self.bbox();
        // Living entities step up like their own movement (default step height).
        let step = if self.e.class == crate::entities::Kind::Other { 0.0 } else { 0.6 };
        let moved = rc_physics::collide::resolve(self.w, &start, d, self.e.on_ground, step);
        let length = |v: [f64; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        // Fully blocked: the game skips only the position change; the ground flag and the blocked
        // axes' speed below still follow the collision.
        let blocked = length(moved) <= 1.0e-7 && length(d) - length(moved) >= 1.0e-7;
        if !blocked {
            self.move_by(start, moved);
        }
        if d[1] != 0.0 {
            self.e.on_ground = d[1] < 0.0 && moved[1] != d[1];
        }
        // What moves itself here loses its speed on a blocked axis, as in the game.
        if self.moves_itself() {
            (0..3).filter(|&i| (moved[i] - d[i]).abs() > 1.0e-5).for_each(|i| self.e.velocity[i] = 0.0);
        }
    }
}

impl TrackedTarget<'_> {
    /// A projectile or the piloted vehicle: simulated on this side.
    fn moves_itself(&self) -> bool {
        self.piloted_here || moves_itself(self.e)
    }

    /// The entity moved by `moved` from its box `start`; the interpolation target follows where
    /// the entity's box finds room.
    fn move_by(&mut self, start: Aabb, moved: [f64; 3]) {
        let before = self.e.pos;
        (0..3).for_each(|i| self.e.pos[i] += moved[i]);
        let w = self.w;
        let box_at = |p: [f64; 3]| Aabb {
            min: std::array::from_fn(|i| start.min[i] + p[i] - before[i]),
            max: std::array::from_fn(|i| start.max[i] + p[i] - before[i]),
        };
        self.e.add_predicted_movement(moved, |p| rc_physics::is_free(w, &box_at(p)));
    }
}

/// A projectile: it moves itself on this side (`projectile_sim`).
fn moves_itself(e: &crate::entities::Entity) -> bool {
    e.facts.motion != crate::entities::MotionFamily::None
}

/// Pushed here without being a simulated body: not riding, not hanging on a block (only the
/// server knocks those off), not ignored by pistons (markers, effect clouds, displays,
/// interactions, item spawners, marker armor stands).
fn pushed_as_tracked(e: &crate::entities::Entity) -> bool {
    let ignored = matches!(
        e.kind.as_str(),
        "painting" | "item_frame" | "glow_item_frame" | "leash_knot" | "marker" | "area_effect_cloud" | "interaction"
            | "ominous_item_spawner" | "block_display" | "item_display" | "text_display"
    );
    // Armor stand flags (data 15): 0x10 is the marker.
    let marker_stand = e.kind == "armor_stand" && e.meta_byte(15) & 0x10 != 0;
    e.motion.is_none() && e.vehicle.is_none() && !entity_motion::simulated(&e.kind) && !ignored && !marker_stand
}

impl App {
    /// World event (sound, particles, effects), received or replayed locally.
    pub(crate) fn on_level_event(&mut self, kind: i32, pos: [i32; 3], data: i32, global: bool) {
        if let Some(a) = self.audio.get() {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let here = view.state(pos[0], pos[1], pos[2]);
            let r = &mut self.world_random;
            world_sounds::level_event(a, &self.data, r, kind, pos, data, global, self.eye, here);
        }
        self.act_level_particles(kind, pos, data);
        self.fx_level_event(kind, pos, data);
    }

    /// Block placed locally (replay), without prediction; local relight always.
    fn piston_set_block(&mut self, p: [i32; 3], state: u32, origin: ChangeOrigin) {
        let (cx, cz) = (p[0] >> 4, p[2] >> 4);
        let sy = (p[1] - self.world.min_y) >> 4;
        let idx = rc_world::block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize);
        let change = [(idx as u16, state)];
        self.update_section_lit(cx, cz, sy, &change, |s, t| t.is_air(s), origin);
    }

    /// `block_event` of a piston: local replay of the trigger.
    pub(crate) fn piston_event(&mut self, pos: [i32; 3], b0: u8, b1: u8) {
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        if !self.piston_replay.is_piston(view.state(pos[0], pos[1], pos[2])) {
            return;
        }
        let w = ClientWorld { view, bes: &self.block_entities, bounds: self.weather.border.bounds() };
        let ops = self.piston_replay.replay(&w, pos, b0, b1);
        for op in ops {
            match op {
                ReplayOp::Set(p, s) => self.piston_set_block(p, s, ChangeOrigin::PistonEvent),
                ReplayOp::Mover(p, m) => self.block_entities.set_mover(p, m),
                ReplayOp::Destroyed(p, s) => self.on_level_event(2001, p, s as i32, false),
            }
        }
    }

    /// Step of the moving blocks of this tick (once, after the local player's tick).
    pub(crate) fn tick_pistons(&mut self) {
        if !std::mem::take(&mut self.pistons_due) {
            return;
        }
        let mut list = Vec::new();
        self.block_entities.tick_pistons(&mut list);
        self.piston_ticks(&list);
    }

    /// Entities pushed by the block at `pos` on this side: the bodies simulated here, then the
    /// tracked ones (mobs, other players, boats, minecarts, armor stands, projectiles).
    fn push_entities(&mut self, pos: [i32; 3], m: &Moving, next: f32, time: i64) {
        let sweep = self.phys.piston_sweep(pos, m, next);
        let carry = self.phys.honey_carry(pos, m, next);
        let areas: Vec<rc_physics::Aabb> = sweep.iter().map(|s| s.area).chain(carry.iter().map(|c| c.0)).collect();
        if areas.is_empty() {
            return;
        }
        let push = Push { pos, m, sweep: sweep.as_ref(), carry: carry.as_ref(), areas: &areas, time };
        self.push_bodies(&push);
        self.push_tracked(&push);
    }

    /// Items, orbs, TNT and falling blocks simulated here, pushed by the block at `push.pos`.
    fn push_bodies(&mut self, push: &Push) {
        let hit = |e: &crate::entities::Entity| {
            e.vehicle.is_none()
                && e.motion.as_ref().is_some_and(|b| push.areas.iter().any(|a| a.intersects(&b.bounding_box())))
        };
        if !self.entities.map.values().any(hit) {
            return;
        }
        let colliders = entity_motion::colliders(&self.entities, None, None);
        let world = GridWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            t: &self.phys,
            border: &self.weather.border,
            colliders: &colliders,
            movers: Some(&self.block_entities),
            noclip: Some(push.m.movement()),
            palette_prefilter: crate::grid_world::palette_prefilter(),
        };
        for e in self.entities.map.values_mut().filter(|e| e.vehicle.is_none()) {
            let k = entity_motion::heft(&e.kind);
            let Some(b) = e.motion.as_deref_mut() else { continue };
            (b.pos, b.vel, b.on_ground) = (e.pos, e.velocity, e.on_ground);
            let mut target = BodyTarget { b, w: &world, t: &self.phys, time: push.time, k };
            push.apply(&mut target);
            (e.pos, e.velocity, e.on_ground) = (target.b.pos, target.b.vel, target.b.on_ground);
        }
    }

    /// Entities the client does not simulate as bodies, pushed by the block at `push.pos` like the
    /// game moves them on its side too: with collisions under their own cap of the tick, the push
    /// added to their interpolation. The list is gathered first: each one collides with the solid
    /// entities but itself.
    fn push_tracked(&mut self, push: &Push) {
        let dims = &self.entities.dims;
        // Cheap flags first, then the area test (the entity's cached box; never for an item frame,
        // whose box depends on its map and which is not pushed here anyway), the kind rules only
        // for the few inside.
        let touched: Vec<i32> = (self.entities.map.iter())
            .filter(|(_, e)| e.motion.is_none() && e.vehicle.is_none() && !e.facts.item_frame)
            .filter(|(_, e)| {
                let b = e.cached_box(dims, &|_| None, &|_| false);
                push.areas.iter().any(|a| a.intersects(&b))
            })
            .filter(|(_, e)| pushed_as_tracked(e))
            .map(|(&id, _)| id)
            .collect();
        for id in touched {
            let colliders = entity_motion::colliders(&self.entities, Some(id), None);
            let world = GridWorld {
                grid: &self.world.grid,
                min_y: self.world.min_y,
                height: self.world.height,
                t: &self.phys,
                border: &self.weather.border,
                colliders: &colliders,
                movers: Some(&self.block_entities),
                noclip: Some(push.m.movement()),
                palette_prefilter: crate::grid_world::palette_prefilter(),
            };
            let Some(e) = self.entities.map.get_mut(&id) else { continue };
            // The piloted vehicle's position is read back from the entity at its next tick (the
            // push is kept and sent with it); its physics also takes the speed and ground flag.
            let piloted_here = self.riding.simulated_root() == Some(id);
            push.apply(&mut TrackedTarget { e, dims: &self.entities.dims, w: &world, time: push.time, piloted_here });
            if piloted_here {
                self.riding.pushed_by_piston(id, e.velocity, e.on_ground);
            }
        }
    }

    /// Pushes and ends of the moving blocks of the tick.
    pub(crate) fn piston_ticks(&mut self, list: &[PistonTick]) {
        if list.is_empty() {
            return;
        }
        let time = self.env.env.clocks.game_time;
        for &t in list {
            match t {
                PistonTick::Push(pos, m, next) => {
                    self.push_entities(pos, &m, next, time);
                    // Area touched by the block this tick: within 1.5 blocks of its cell (push
                    // of at most 0.5, honey up to 1.5 above).
                    let p = pos.map(f64::from);
                    let near = rc_physics::Aabb { min: p.map(|v| v - 1.5), max: p.map(|v| v + 2.5) };
                    if !self.positioned || !self.player.bounding_box().intersects(&near) {
                        continue;
                    }
                    let colliders = entity_motion::colliders(&self.entities, None, Some(self.player.pos[1]));
                    let world = GridWorld {
                        grid: &self.world.grid,
                        min_y: self.world.min_y,
                        height: self.world.height,
                        t: &self.phys,
                        border: &self.weather.border,
                        colliders: &colliders,
                        movers: Some(&self.block_entities),
                        noclip: Some(m.movement()),
                        palette_prefilter: crate::grid_world::palette_prefilter(),
                    };
                    let before = self.player.pos;
                    self.player.piston_tick(pos, &m, next, time, &world, &self.phys);
                    // The push is a movement of the player's own: its footsteps count it.
                    let moved: [f64; 3] = std::array::from_fn(|i| self.player.pos[i] - before[i]);
                    if moved != [0.0; 3]
                        && let Some(a) = self.audio.get()
                    {
                        self.world_sounds.movement_steps(a, &self.data, &self.phys, &world, &self.player, moved);
                    }
                }
                PistonTick::Finish(pos, moved) => {
                    let view = environ::WorldView {
                        grid: &self.world.grid,
                        min_y: self.world.min_y,
                        height: self.world.height,
                    };
                    if self.phys.is_moving_piston(view.state(pos[0], pos[1], pos[2])) {
                        self.piston_set_block(pos, moved, ChangeOrigin::PistonFinish);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::{Entities, Entity};
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::{AddEntity, Meta};

    /// Full blocks from x = 2 eastwards, air elsewhere.
    struct WallFromX2;

    impl rc_physics::World for WallFromX2 {
        fn state(&self, _: i32, _: i32, _: i32) -> Option<u32> {
            Some(0)
        }
        fn collision(&self, x: i32, _: i32, _: i32, out: &mut Vec<Aabb>) -> bool {
            if x >= 2 {
                out.push(Aabb { min: [0.0; 3], max: [1.0; 3] });
            }
            false
        }
    }

    /// Entity `kind` spawned at `pos`, of width 0.9 and height 0.9.
    fn spawn(kind: &str, pos: [f64; 3]) -> Entity {
        let mut all = Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
        let add = AddEntity { id: 1, uuid: 1, kind: 0, pos, velocity: [0.0; 3], pitch: 0.0, yaw: 0.0, head_yaw: 0.0, data: 0 };
        all.apply(EntityEvent::Add(Box::new(add)));
        all.map.remove(&1).expect("spawned")
    }

    const PIG_SIZE: [[f32; 4]; 1] = [[0.9, 0.9, 0.8, 0.9]];

    /// A pig shown here, pushed twice in one tick, moves by its cap of 0.51; over the next ticks
    /// it moves on until the wall stops it, its box against x = 2. Before: the client never
    /// moved it.
    #[test]
    fn tracked_entity_push_is_capped_and_collides() {
        let mut pig = spawn("pig", [0.0, 64.0, 0.5]);
        for _ in 0..2 {
            TrackedTarget { e: &mut pig, dims: &PIG_SIZE, w: &WallFromX2, time: 1, piloted_here: false }.piston_push([0.4, 0.0, 0.0]);
        }
        assert!((pig.pos[0] - 0.51).abs() < 1e-9, "capped within the tick: {}", pig.pos[0]);
        for time in 2..=5 {
            TrackedTarget { e: &mut pig, dims: &PIG_SIZE, w: &WallFromX2, time, piloted_here: false }.piston_push([0.4, 0.0, 0.0]);
        }
        assert!((pig.pos[0] - 1.55).abs() < 1e-6, "stopped by the wall: {}", pig.pos[0]);
    }

    /// A push the wall stops entirely still stops what moves itself on its blocked axis (a
    /// projectile, the piloted vehicle), as the game's movement does; a mob shown here keeps its
    /// speed. Before: a fully blocked push changed nothing at all.
    #[test]
    fn fully_blocked_push_stops_what_moves_itself() {
        let blocked_push = |kind: &str, piloted_here: bool| {
            let mut e = spawn(kind, [1.55, 64.0, 0.5]);
            e.velocity = [0.3, 0.0, 0.0];
            TrackedTarget { e: &mut e, dims: &PIG_SIZE, w: &WallFromX2, time: 1, piloted_here }.piston_push([0.4, 0.0, 0.0]);
            assert_eq!(e.pos[0], 1.55, "{kind}: blocked, not moved");
            e.velocity[0]
        };
        assert_eq!(blocked_push("snowball", false), 0.0, "projectile");
        assert_eq!(blocked_push("pig", true), 0.0, "piloted vehicle");
        assert_eq!(blocked_push("pig", false), 0.3, "mob shown here");
    }

    /// What the client pushes besides its simulated bodies: mobs and projectiles, not a painting
    /// (only the server knocks it off), nor a marker armor stand, nor an item (a body).
    #[test]
    fn entities_pushed_as_tracked() {
        assert!(pushed_as_tracked(&spawn("pig", [0.0; 3])));
        assert!(pushed_as_tracked(&spawn("snowball", [0.0; 3])));
        assert!(pushed_as_tracked(&spawn("armor_stand", [0.0; 3])));
        assert!(!pushed_as_tracked(&spawn("painting", [0.0; 3])));
        assert!(!pushed_as_tracked(&spawn("item", [0.0; 3])));
        let mut marker = spawn("armor_stand", [0.0; 3]);
        marker.meta.insert(15, Meta::Byte(0x10));
        assert!(!pushed_as_tracked(&marker));
    }
}