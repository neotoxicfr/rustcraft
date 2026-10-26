//! Pistons on the client side (`block-entities.md` § 1.4): the server does not send the moving
//! blocks, the client recreates them by replaying the piston's block event (scheduled by
//! `rc_sim::PistonReplay`, same resolution as the server), then their tick pushes the local
//! player and places the final block if it has not arrived from the server in the meantime.

use rc_physics::piston::{BodyTarget, Moving, carry_stuck, push_collided};
use rc_sim::{MovingBlock, PistonWorld, ReplayOp, ReplayWorld};

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

    /// Block placed locally (replay), without prediction.
    fn piston_set_block(&mut self, p: [i32; 3], state: u32) {
        let (cx, cz) = (p[0] >> 4, p[2] >> 4);
        let sy = (p[1] - self.world.min_y) >> 4;
        let idx = rc_world::block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize);
        self.update_section(cx, cz, sy, &[(idx as u16, state)], |s, t| t.air.get(s as usize).copied().unwrap_or(true));
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
                ReplayOp::Set(p, s) => self.piston_set_block(p, s),
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

    /// Items, orbs, TNT and falling blocks simulated here, pushed by the block at `pos`.
    fn push_bodies(&mut self, pos: [i32; 3], m: &Moving, next: f32, time: i64) {
        let sweep = self.phys.piston_sweep(pos, m, next);
        let carry = self.phys.honey_carry(pos, m, next);
        let areas: Vec<rc_physics::Aabb> = sweep.iter().map(|s| s.area).chain(carry.iter().map(|c| c.0)).collect();
        let hit = |e: &crate::entities::Entity| {
            e.vehicle.is_none()
                && e.motion.as_ref().is_some_and(|b| areas.iter().any(|a| a.intersects(&b.bounding_box())))
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
            noclip: Some(m.movement()),
        };
        for e in self.entities.map.values_mut().filter(|e| e.vehicle.is_none()) {
            let g = if e.kind == "experience_orb" { 0.03 } else { 0.04 };
            let Some(b) = e.motion.as_deref_mut() else { continue };
            (b.pos, b.vel, b.on_ground) = (e.pos, e.velocity, e.on_ground);
            let mut target = BodyTarget { b, w: &world, t: &self.phys, time, g };
            if let Some(sw) = &sweep {
                push_collided(sw, pos, &mut target);
            }
            if let Some(c) = &carry {
                carry_stuck(c, pos, &mut target);
            }
            (e.pos, e.velocity, e.on_ground) = (target.b.pos, target.b.vel, target.b.on_ground);
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
                    self.push_bodies(pos, &m, next, time);
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
                    };
                    self.player.piston_tick(pos, &m, next, time, &world, &self.phys);
                }
                PistonTick::Finish(pos, moved) => {
                    let view = environ::WorldView {
                        grid: &self.world.grid,
                        min_y: self.world.min_y,
                        height: self.world.height,
                    };
                    if self.phys.is_moving_piston(view.state(pos[0], pos[1], pos[2])) {
                        self.piston_set_block(pos, moved);
                    }
                }
            }
        }
    }
}
