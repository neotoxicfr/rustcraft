//! Pistons (§ 3.9): quasi-connectivity, extend and retract events (fast retraction),
//! structure resolution (lines, sticky branches, limit of 12), movement with the
//! game's hashed table, moving blocks (ticking block entity), heads.

use rc_data::sim_bits;

use super::level::Mover;
use super::piston_plan::{PistonWorld, Resolver};
use super::{Dir, JavaMap, Kind, P, above, flag, rel, rel_n};
use crate::mobs::mob::Mob;
use crate::server::Server;
use rc_physics::Aabb;
use rc_physics::piston::{BodyTarget, Moving, PistonTarget, Sweep, carry_stuck, push_collided};

/// Mob pushed by a piston (`move(PISTON)`: movement with server collisions).
struct MobTarget<'a> {
    s: &'a mut Server,
    m: &'a mut Mob,
}

impl PistonTarget for MobTarget<'_> {
    fn bbox(&self) -> Aabb {
        self.m.bbox()
    }
    fn position(&self) -> [f64; 3] {
        self.m.pos
    }
    fn grounded(&self) -> bool {
        self.m.on_ground
    }
    fn supported_by(&self, p: [i32; 3]) -> bool {
        self.m.support == Some(p)
    }
    fn launch(&mut self, axis: usize, v: f64) {
        self.m.vel[axis] = v;
        self.m.needs_sync = true;
    }
    fn piston_push(&mut self, d: [f64; 3]) {
        // Without physics (vex): placed as is, no ceiling.
        if crate::mobs::illagers::no_physics(self.m) {
            for (p, v) in self.m.pos.iter_mut().zip(d) {
                *p += v;
            }
            return;
        }
        let d = self.m.piston_deltas.limit(self.s.time, d);
        if d != [0.0; 3] {
            self.s.mob_move(self.m, d);
        }
    }
}

impl PistonWorld for Server {
    #[inline]
    fn state(&self, p: P) -> u32 {
        self.get(p)
    }
    fn min_y(&self) -> i32 {
        self.dim.height.min_y
    }
    fn max_y(&self) -> i32 {
        self.top_y() - 1
    }
    fn in_border(&self, p: P) -> bool {
        Self::block_in_border(self.border_bounds(), p)
    }
}

impl Server {
    fn piston_state(&self, sticky: bool) -> u32 {
        let t = &self.bs.t;
        self.v().default_state(if sticky { t.id.sticky_piston } else { t.id.piston })
    }

    /// Power by quasi-connectivity.
    fn piston_powered(&self, p: P, push: Dir) -> bool {
        for d in Dir::ALL {
            if d != push && self.has_signal(rel(p, d), d) {
                return true;
            }
        }
        if self.has_signal(p, Dir::Down) {
            return true;
        }
        let a = above(p);
        Dir::ALL.iter().any(|&d| d != Dir::Down && self.has_signal(rel(a, d), d))
    }

    /// Check.
    pub(crate) fn piston_check(&mut self, p: P, s: u32) {
        let v = self.v();
        let dir = v.facing(s);
        let extend = self.piston_powered(p, dir);
        let extended = v.get_bool(s, "extended");
        let b = v.block(s);
        if extend && !extended {
            if (Resolver { v: &v, w: self }).resolve(p, dir, true).is_some() {
                self.block_event(p, b, 0, dir as i32);
            }
        } else if !extend && extended {
            let pushed = rel_n(p, dir, 2);
            let ps = self.get(pushed);
            let mut event = 1;
            if self.v().kind(ps) == Kind::MovingPiston
                && self.v().facing(ps) == dir
                && let Some(m) = self.bs.movers.get(&pushed)
                && m.extending
                && (m.progress_o < 0.5 || self.time == m.last_ticked || self.bs.handling_tick)
            {
                event = 2;
            }
            self.block_event(p, b, event, dir as i32);
        }
    }

    pub(crate) fn piston_trigger(&mut self, s: u32, p: P, a: i32, b: i32, sticky: bool) -> bool {
        let dir = self.v().facing(s);
        let extended = self.v().set_bool(s, "extended", true);
        let extend = self.piston_powered(p, dir);
        if extend && (a == 1 || a == 2) {
            self.set_block(p, extended, flag::CLIENTS);
            return false;
        }
        if !extend && a == 0 {
            return false;
        }
        if a == 0 {
            if !self.move_blocks(p, dir, true, sticky) {
                return false;
            }
            self.set_block(p, extended, flag::NEIGHBORS | flag::CLIENTS | flag::MOVED);
            let pitch = self.bs.rng.next_float() * 0.25 + 0.6;
            self.block_sound(p, "minecraft:block.piston.extend", 0.5, pitch);
            self.game_event_block("minecraft:block_activate", p, None, Some(extended));
        } else if a == 1 || a == 2 {
            let head = rel(p, dir);
            if self.bs.movers.contains_key(&head) {
                self.mover_final_tick(head);
            }
            let mp = self.moving_piston_state(dir, sticky);
            self.set_block(p, mp, flag::INVISIBLE | flag::KNOWN_SHAPE | flag::SKIP_BE_SIDE_EFFECTS);
            let base = self.v().set_dir(self.piston_state(sticky), "facing", Dir::from_index((b & 7) as usize));
            self.add_mover(p, base, dir, false, true);
            let mpb = self.bs.t.id.moving_piston;
            self.update_neighbors_at(p, mpb);
            self.update_neighbour_shapes(mp, p, flag::CLIENTS, 512);
            if sticky {
                let two = rel_n(p, dir, 2);
                let ms = self.get(two);
                let mut piece = false;
                if self.v().kind(ms) == Kind::MovingPiston
                    && let Some(m) = self.bs.movers.get(&two)
                    && m.dir == dir
                    && m.extending
                {
                    self.mover_final_tick(two);
                    piece = true;
                }
                if !piece {
                    let v = self.v();
                    let pull_ok = a == 1
                        && !v.is_air(ms)
                        && (Resolver { v: &v, w: self }).pushable(ms, two, dir.opposite(), false, dir)
                        && (v.push_reaction(ms) == sim_bits::PUSH_NORMAL || matches!(v.kind(ms), Kind::Piston { .. }));
                    if pull_ok {
                        self.move_blocks(p, dir, false, sticky);
                    } else {
                        self.remove_block(head, false);
                    }
                }
            } else {
                self.remove_block(head, false);
            }
            let pitch = self.bs.rng.next_float() * 0.15 + 0.6;
            self.block_sound(p, "minecraft:block.piston.contract", 0.5, pitch);
            self.game_event_block("minecraft:block_deactivate", p, None, Some(mp));
        }
        true
    }

    fn moving_piston_state(&self, dir: Dir, sticky: bool) -> u32 {
        let v = self.v();
        let s = v.set_dir(v.default_state(self.bs.t.id.moving_piston), "facing", dir);
        v.set(s, "type", if sticky { "sticky" } else { "normal" })
    }

    fn move_blocks(&mut self, piston: P, dir: Dir, extending: bool, sticky: bool) -> bool {
        let arm = rel(piston, dir);
        if !extending && self.v().kind(self.get(arm)) == Kind::PistonHead {
            let air = self.air();
            self.set_block(arm, air, flag::INVISIBLE | flag::KNOWN_SHAPE | flag::SKIP_BE_SIDE_EFFECTS);
        }
        let Some(st) = (Resolver { v: &self.v(), w: self }).resolve(piston, dir, extending) else { return false };
        let mut delete: JavaMap<u32> = JavaMap::new();
        let mut shapes = Vec::with_capacity(st.to_push.len());
        for &q in &st.to_push {
            let s = self.get(q);
            shapes.push(s);
            delete.insert(q, s);
        }
        let push_dir = if extending { dir } else { dir.opposite() };
        let mut to_update: Vec<u32> = Vec::new();
        for &q in st.to_destroy.iter().rev() {
            let s = self.get(q);
            // Loot of the crushed block (no tool); the server sends no break effect:
            // the client replays the piston.
            self.drop_resources(q, s, None);
            let air = self.air();
            self.set_block(q, air, flag::CLIENTS | flag::KNOWN_SHAPE);
            let c = [f64::from(q[0]) + 0.5, f64::from(q[1]) + 0.5, f64::from(q[2]) + 0.5];
            self.game_event_src("minecraft:block_destroy", c, None, Some(s));
            to_update.push(s);
        }
        for i in (0..st.to_push.len()).rev() {
            let q = st.to_push[i];
            let s = self.get(q);
            let t = rel(q, push_dir);
            delete.remove(t);
            let mp = self.moving_piston_state(dir, false);
            let mp = self.v().set(mp, "type", "normal");
            self.set_block(t, mp, flag::INVISIBLE | flag::MOVED | flag::SKIP_BE_SIDE_EFFECTS);
            self.add_mover(t, shapes[i], dir, extending, false);
            to_update.push(s);
        }
        if extending {
            let v = self.v();
            let head = v.set_dir(v.default_state(self.bs.t.id.piston_head), "facing", dir);
            let head = v.set(head, "type", if sticky { "sticky" } else { "normal" });
            let mp = self.moving_piston_state(dir, sticky);
            delete.remove(arm);
            self.set_block(arm, mp, flag::INVISIBLE | flag::MOVED | flag::SKIP_BE_SIDE_EFFECTS);
            self.add_mover(arm, head, dir, true, true);
        }
        let air = self.air();
        let entries: Vec<(P, u32)> = delete.iter().copied().collect();
        for &(q, _) in &entries {
            self.set_block(q, air, flag::CLIENTS | flag::KNOWN_SHAPE | flag::MOVED);
        }
        for &(q, old) in &entries {
            self.update_indirect_shapes(old, q, flag::CLIENTS, 512);
            self.update_neighbour_shapes(air, q, flag::CLIENTS, 512);
            self.update_indirect_shapes(air, q, flag::CLIENTS, 512);
        }
        let mut k = 0;
        for &q in st.to_destroy.iter().rev() {
            let s = to_update[k];
            k += 1;
            self.affect_after_removal(s, q, false);
            self.update_indirect_shapes(s, q, flag::CLIENTS, 512);
            let b = self.v().block(s);
            self.update_neighbors_at(q, b);
        }
        for &q in st.to_push.iter().rev() {
            let b = self.v().block(to_update[k]);
            k += 1;
            self.update_neighbors_at(q, b);
        }
        if extending {
            let hb = self.bs.t.id.piston_head;
            self.update_neighbors_at(arm, hb);
        }
        true
    }

    fn add_mover(&mut self, p: P, moved: u32, dir: Dir, extending: bool, source: bool) {
        self.remove_block_entity(p);
        let id = self.add_ticker(p);
        self.bs.movers.insert(
            p,
            Mover { id, moved, dir, extending, source, progress: 0.0, progress_o: 0.0, last_ticked: i64::MIN },
        );
    }

    /// Forced finalisation (§ 3.9).
    pub(crate) fn mover_final_tick(&mut self, p: P) {
        let Some(m) = self.bs.movers.get(&p).copied() else { return };
        if m.progress_o >= 1.0 {
            return;
        }
        self.bs.movers.remove(&p);
        if self.v().kind(self.get(p)) == Kind::MovingPiston {
            let ns = if m.source { self.air() } else { self.update_from_neighbour_shapes(m.moved, p) };
            self.set_update(p, ns);
            let b = self.v().block(ns);
            self.neighbor_changed_at(p, b);
        }
    }

    /// Entities touched by the moving block at `p` going to `next`: pushed by the
    /// swept area (mobs, items, orbs, primed TNT, falling blocks), then dragged
    /// by honey. The player is pushed by their client, which replays the piston. Projectiles and
    /// vehicles: not yet (`docs/gaps-protocol.md`).
    fn mover_push_entities(&mut self, p: P, m: &Moving, next: f32) {
        let sweep = self.phys.piston_sweep(p, m, next);
        let carry = self.phys.honey_carry(p, m, next);
        let prev = self.bs.noclip.replace(m.movement());
        // All pushed entities first, then those dragged by honey.
        if let Some(sw) = &sweep {
            self.push_targets(sw.area, Some(sw), None, p);
        }
        if let Some(c) = &carry {
            self.push_targets(c.0, None, Some(c), p);
        }
        self.bs.noclip = prev;
    }

    /// Mobs, items, orbs, TNT and falling blocks whose box touches `area`, pushed
    /// or dragged.
    fn push_targets(&mut self, area: Aabb, sw: Option<&Sweep>, carry: Option<&(Aabb, u8, f64)>, p: P) {
        let apply = |e: &mut dyn PistonTarget| {
            if let Some(sw) = sw {
                push_collided(sw, p, e);
            }
            if let Some(c) = carry {
                carry_stuck(c, p, e);
            }
        };
        for i in 0..self.mobs.list.len() {
            if !self.mobs.list[i].as_ref().is_some_and(|o| !o.removed && o.touches(&area)) {
                continue;
            }
            let Some(mut o) = self.mobs.list[i].take() else { continue };
            apply(&mut MobTarget { s: self, m: &mut o });
            self.mobs.list[i] = Some(o);
        }
        let time = self.time;
        let mut drops = std::mem::take(&mut self.life.drops);
        let mut orbs = std::mem::take(&mut self.life.orbs);
        let mut ents = std::mem::take(&mut self.bs.entities.list);
        {
            let w = self.world();
            // Primed TNT and falling blocks (the game's default "push" reaction).
            for e in ents.iter_mut().filter(|e| !e.removed) {
                let mut b = self.body_of(e);
                if b.bounding_box().intersects(&area) {
                    apply(&mut BodyTarget { b: &mut b, w: &w, t: &self.phys, time, g: 0.04 });
                }
                Self::put_body(e, b);
            }
            for d in drops.iter_mut().filter(|d| !d.is_removed() && d.body.bounding_box().intersects(&area)) {
                let b = &mut d.body;
                (b.pos, b.vel, b.on_ground) = (d.pos, d.vel, d.on_ground);
                apply(&mut BodyTarget { b, w: &w, t: &self.phys, time, g: 0.04 });
                (d.pos, d.vel, d.on_ground) = (d.body.pos, d.body.vel, d.body.on_ground);
            }
            for o in orbs.iter_mut().filter(|o| !o.is_removed() && o.body.bounding_box().intersects(&area)) {
                let b = &mut o.body;
                (b.pos, b.vel, b.on_ground) = (o.pos, o.vel, o.on_ground);
                apply(&mut BodyTarget { b, w: &w, t: &self.phys, time, g: 0.03 });
                (o.pos, o.vel, o.on_ground) = (o.body.pos, o.body.vel, o.body.on_ground);
            }
        }
        self.life.drops = drops;
        self.life.orbs = orbs;
        self.bs.entities.list = ents;
    }

    /// Tick of the moving block (block entity phase).
    pub(crate) fn mover_tick(&mut self, p: P) {
        let Some(m) = self.bs.movers.get_mut(&p) else { return };
        m.last_ticked = self.time;
        m.progress_o = m.progress;
        if m.progress_o >= 1.0 {
            let m = *m;
            self.bs.movers.remove(&p);
            if self.v().kind(self.get(p)) == Kind::MovingPiston {
                let ns = self.update_from_neighbour_shapes(m.moved, p);
                if self.v().is_air(ns) {
                    self.set_block(p, m.moved, 340);
                    self.update_or_destroy(m.moved, ns, p, flag::ALL, 512);
                } else {
                    let ns = if self.v().get_bool(ns, "waterlogged") {
                        self.v().set_bool(ns, "waterlogged", false)
                    } else {
                        ns
                    };
                    self.set_block(p, ns, flag::NEIGHBORS | flag::CLIENTS | flag::MOVED);
                    let b = self.v().block(ns);
                    self.neighbor_changed_at(p, b);
                }
            }
        } else {
            let next = m.progress + 0.5;
            let phys = m.physics();
            self.mover_push_entities(p, &phys, next);
            if let Some(m) = self.bs.movers.get_mut(&p) {
                m.progress = next.min(1.0);
            }
        }
    }

    pub(crate) fn piston_head_fitting(&self, head: u32, base: u32) -> bool {
        let v = self.v();
        let want = if v.get(head, "type") == Some("sticky") { self.bs.t.id.sticky_piston } else { self.bs.t.id.piston };
        v.block(base) == want && v.get_bool(base, "extended") && v.facing(base) == v.facing(head)
    }

    pub(crate) fn piston_head_survives(&self, head: u32, p: P) -> bool {
        let v = self.v();
        let base = self.get(rel(p, v.facing(head).opposite()));
        self.piston_head_fitting(head, base) || (v.kind(base) == Kind::MovingPiston && v.facing(base) == v.facing(head))
    }

    pub(crate) fn piston_head_removed(&mut self, head: u32, p: P) {
        let bp = rel(p, self.v().facing(head).opposite());
        if self.piston_head_fitting(head, self.get(bp)) {
            self.destroy_block(bp, true, 512);
        }
    }

    pub(crate) fn piston_head_neighbor_changed(&mut self, s: u32, p: P, from: u16) {
        if self.piston_head_survives(s, p) {
            let bp = rel(p, self.v().facing(s).opposite());
            self.neighbor_changed_at(bp, from);
        }
    }

    /// Piston placed by a player.
    pub(crate) fn piston_placed_by_player(&mut self, p: P, s: u32) {
        self.piston_check(p, s);
    }
}
