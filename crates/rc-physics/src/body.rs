//! Small entities that the game simulates on both sides (`docs/specs/entities-objects.md` "client-side
//! movement"): dropped items, experience orbs, falling blocks, primed TNT. The
//! server only sends back their position every 10 or 20 ticks; between two re-syncs, the
//! client runs the same tick itself (gravity, movement with collisions, friction, bounces,
//! effects of the blocks crossed, push of the currents) so that the entity moves on screen.
//!
//! Also: ray cast against the collision shapes of the blocks (projectiles).

use crate::{Aabb, BUBBLES_DOWN, Fluid, Tables, World, below_of, collide, inside, l2, scan_fluids, sub, support_of};

mod bobber;
pub use bobber::{Bobber, BobberState, BobberTick};

/// The game's `f32` value seen as `f64` (the game's floating constants are widened this way).
fn f(x: f32) -> f64 {
    f64::from(x)
}

/// Movement of the tick: start, end, wanted movement (order of the swept axes).
type Move = ([f64; 3], [f64; 3], Option<[f64; 3]>);

/// Player that an orb can follow: identifier, feet, eye height, spectator, dying.
#[derive(Debug, Clone, Copy)]
pub struct Follow {
    pub id: i32,
    pub pos: [f64; 3],
    pub eye: f64,
    pub spectator: bool,
    pub dead: bool,
}

impl Follow {
    /// Alive and not a spectator.
    pub fn eligible(&self) -> bool {
        !self.spectator && !self.dead
    }
}

/// Physical state of a small entity. Position and velocity are copied from the displayed
/// entity before each tick (the server may have re-synced them) and handed back afterwards.
#[derive(Debug, Clone)]
pub struct Body {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub width: f64,
    pub height: f64,
    pub eye: f64,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub vertical_below: bool,
    /// "No gravity" data (index 5).
    pub no_gravity: bool,
    /// Fast lava in this dimension.
    pub fast_lava: bool,
    /// Water and lava heights touched, eyes in water (last update).
    pub water: f64,
    pub lava: f64,
    pub eyes_in_water: bool,
    /// Movement actually done at the last `travel` (prediction of the interpolated orbs).
    pub moved: [f64; 3],
    /// Movement without collision (item stuck in a block, server side).
    pub no_physics: bool,
    /// Server side (authoritative instance): ground recomputed at every movement, even
    /// without a vertical component; orbs: no tick skipped at spawn.
    pub server: bool,
    /// Accumulated fall distance (held by the server for items and orbs).
    pub fall: f64,
    old: [f64; 3],
    support: Option<[i32; 3]>,
    no_blocks: bool,
    stuck: [f64; 3],
    /// Piston pushes accumulated during the tick.
    piston_deltas: crate::piston::PistonDeltas,
    moves: Vec<Move>,
    last_moves: Vec<Move>,
    first_tick: bool,
    following: Option<i32>,
}

impl Body {
    pub fn new(pos: [f64; 3], vel: [f64; 3], width: f64, height: f64, eye: f64) -> Body {
        Body {
            pos,
            vel,
            width,
            height,
            eye,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_below: false,
            no_gravity: false,
            fast_lava: false,
            water: 0.0,
            lava: 0.0,
            eyes_in_water: false,
            moved: [0.0; 3],
            no_physics: false,
            server: false,
            fall: 0.0,
            old: pos,
            support: None,
            no_blocks: false,
            stuck: [0.0; 3],
            piston_deltas: Default::default(),
            moves: Vec::new(),
            last_moves: Vec::new(),
            first_tick: true,
            following: None,
        }
    }

    pub fn bounding_box(&self) -> Aabb {
        self.box_at(self.pos)
    }

    fn box_at(&self, p: [f64; 3]) -> Aabb {
        let r = self.width / 2.0;
        Aabb { min: [p[0] - r, p[1], p[2] - r], max: [p[0] + r, p[1] + self.height, p[2] + r] }
    }

    fn state(w: &dyn World, p: [i32; 3]) -> u32 {
        w.state(p[0], p[1], p[2]).unwrap_or(0)
    }

    fn block_pos(&self) -> [i32; 3] {
        self.pos.map(|v| v.floor() as i32)
    }

    /// Block under the feet with an offset (0.2: effect block; 0.500001: slowing block).
    pub fn below(&self, t: &Tables, w: &dyn World, h: f32) -> [i32; 3] {
        below_of(self.support, self.pos, t, w, h)
    }

    /// Main supporting block (last computed after a movement).
    pub fn support(&self) -> Option<[i32; 3]> {
        self.support
    }

    fn gravity(&self, g: f64) -> f64 {
        if self.no_gravity { 0.0 } else { g }
    }

    fn apply_gravity(&mut self, g: f64) {
        let g = self.gravity(g);
        if g != 0.0 {
            self.vel[1] -= g;
        }
    }

    /// Fluids touched and push of the currents (game update, pushed entity).
    fn update_fluids(&mut self, w: &dyn World, t: &Tables) -> bool {
        let b = self.bounding_box();
        let (water, lava, eyes) =
            crate::scan_fluids_entity(w, t, &b, self.pos, self.pos[1] + self.eye, true, self.fast_lava, &mut self.vel);
        self.water = water;
        self.lava = lava;
        self.eyes_in_water = eyes;
        water > 0.0 || lava > 0.0
    }

    fn in_water(&self) -> bool {
        self.water > 0.0
    }

    fn in_lava(&self) -> bool {
        self.lava > 0.0
    }

    /// Main supporting block after a movement `m` (offset search if nothing below).
    fn update_support(&mut self, w: &dyn World, m: [f64; 3]) {
        if self.on_ground {
            let b = self.bounding_box();
            let mut found = support_of(w, &b, self.pos);
            if found.is_some() || self.no_blocks {
                self.support = found;
            } else {
                found = support_of(w, &b.offset(-m[0], 0.0, -m[2]), self.pos);
                self.support = found;
            }
            self.no_blocks = found.is_none();
        } else {
            self.no_blocks = false;
            self.support = None;
        }
    }

    /// Own movement of the entity (`g`: effective gravity, `drag`: air friction,
    /// `slow_h`: offset of the slowing block).
    fn travel(&mut self, w: &dyn World, t: &Tables, g: f64, drag: f32, slow_h: f32) {
        if self.no_physics {
            // Without collision: direct placement, impacts cleared, ground unchanged.
            let (from, d) = (self.pos, self.vel);
            for (p, v) in self.pos.iter_mut().zip(d) {
                *p += v;
            }
            self.moves.push((from, self.pos, Some(d)));
            self.moved = d;
            self.horizontal_collision = false;
            self.vertical_collision = false;
            self.vertical_below = false;
            return;
        }
        self.move_delta(self.vel, w, t, g, drag, slow_h, false);
    }

    /// Piston push (`move(PISTON, d)`) at game tick `time`: cap of 0.51 per axis
    /// and per tick, blocking cancelled without reducing the push; then effects of the blocks crossed by
    /// this movement alone, which is not kept for the tick (the last one recorded is removed).
    /// `w` must ignore the pushing block in its direction. Items and orbs: `base_g` gravity of
    /// the entity (0.04, 0.03), air friction 0.98, slowing block at 0.999999.
    pub fn piston_move(&mut self, d: [f64; 3], time: i64, w: &dyn World, t: &Tables, base_g: f64) {
        let (g, drag, slow_h) = (self.gravity(base_g), 0.98, 0.999_999);
        let from = self.pos;
        if self.no_physics {
            for (p, v) in self.pos.iter_mut().zip(d) {
                *p += v;
            }
        } else {
            // Capped to zero: no movement, but block effects and removal all the same.
            let d = self.piston_deltas.limit(time, d);
            if d != [0.0; 3] {
                self.move_delta(d, w, t, g, drag, slow_h, true);
            }
        }
        self.moves.pop();
        let (moves, last, old) = (std::mem::take(&mut self.moves), std::mem::take(&mut self.last_moves), self.old);
        self.old = from;
        self.block_effects(w, t, false);
        (self.moves, self.last_moves, self.old) = (moves, last, old);
    }

    #[allow(clippy::too_many_arguments)]
    fn move_delta(&mut self, d: [f64; 3], w: &dyn World, t: &Tables, g: f64, drag: f32, slow_h: f32, piston: bool) {
        let mut d = d;
        if l2(self.stuck) > 1.0e-7 {
            if !piston {
                for (v, k) in d.iter_mut().zip(self.stuck) {
                    *v *= k;
                }
            }
            self.stuck = [0.0; 3];
            self.vel = [0.0; 3];
        }
        let start = self.bounding_box();
        let m = collide::resolve(w, &start, d, self.on_ground, 0.0);
        self.moved = [0.0; 3];
        if l2(m) > 1.0e-7 || l2(d) - l2(m) < 1.0e-7 {
            let from = self.pos;
            for (p, v) in self.pos.iter_mut().zip(m) {
                *p += v;
            }
            if self.moves.len() >= 100 {
                let second = self.moves.remove(1);
                self.moves[0].1 = second.1;
                self.moves[0].2 = None;
            }
            self.moves.push((from, self.pos, Some(d)));
            self.moved = m;
        }
        let eps = f(1.0e-5);
        let cx = (d[0] - m[0]).abs() >= eps;
        let cz = (d[2] - m[2]).abs() >= eps;
        self.horizontal_collision = cx || cz;
        let vertical_move = d[1].abs() > 0.0;
        // Server side, the instance is authoritative: ground and vertical impact recomputed at each
        // movement (a horizontal piston push lifts the entity off the ground).
        if vertical_move || self.server {
            self.vertical_collision = d[1] != m[1];
            self.vertical_below = self.vertical_collision && d[1] < 0.0;
            self.on_ground = self.vertical_below;
            self.update_support(w, m);
        }
        let effect = Self::state(w, self.below(t, w, 0.2));
        if (vertical_move && self.vertical_collision) || self.horizontal_collision {
            self.restitute(cx, cz, m, effect, g, drag, t);
        }
        // Speed factor of the block: here, otherwise (outside a water block or bubble column) that
        // of the slowing block.
        let here = Self::state(w, self.block_pos());
        let mut k = t.speed_factor.get(here as usize).copied().unwrap_or(1.0);
        if !t.speed_factor_here_only(here) && k == 1.0 {
            let b = Self::state(w, self.below(t, w, slow_h));
            k = t.speed_factor.get(b as usize).copied().unwrap_or(1.0);
        }
        self.vel[0] *= f(k);
        self.vel[2] *= f(k);
    }

    /// Stop on the blocked axis; bounce on a block that gives back momentum (non-living entity: 0.8×).
    #[allow(clippy::too_many_arguments)]
    fn restitute(&mut self, cx: bool, cz: bool, m: [f64; 3], effect: u32, g: f64, drag: f32, t: &Tables) {
        let v = self.vel;
        let mut r = 0.0;
        if cx {
            self.vel[0] = -v[0] * r;
        }
        if cz {
            self.vel[2] = -v[2] * r;
        }
        if self.vertical_collision {
            if self.vertical_below {
                r = if -v[1] > g && !t.suppresses_bounce(effect) {
                    let block = t.bounce.get(effect as usize).copied().unwrap_or(0.0) * 0.8;
                    f64::max(r, f(block))
                } else {
                    0.0
                };
            }
            let (gc, k) = if r > 0.0 {
                let p = m[1] / v[1];
                (p * g, 1.0 + p * (f(drag) - 1.0))
            } else {
                (0.0, 1.0)
            };
            self.vel[1] = (gc - v[1]) * k * r;
        }
    }

    /// Effects of the blocks crossed during the tick (`last`: replays those of the previous tick).
    fn block_effects(&mut self, w: &dyn World, t: &Tables, last: bool) {
        if !last {
            self.last_moves.clear();
            self.last_moves.append(&mut self.moves);
            match self.last_moves.last() {
                None => self.last_moves.push((self.old, self.pos, None)),
                Some(l) if l2(sub(l.1, self.pos)) > f(9.999_999_4e-11) => {
                    self.last_moves.push((l.1, self.pos, None));
                }
                _ => {}
            }
        }
        if self.on_ground {
            let s = Self::state(w, self.below(t, w, 0.2));
            let vy = self.vel[1].abs();
            if t.inside(s) == inside::SLIME && vy < 0.1 {
                let k = 0.4 + vy * 0.2;
                self.vel[0] *= k;
                self.vel[2] *= k;
            }
        }
        let moves = std::mem::take(&mut self.last_moves);
        let mut seen: Vec<[i32; 3]> = Vec::new();
        for &(from, to, d) in &moves {
            let travel = sub(to, from);
            match d {
                Some(d) if l2(travel) > 0.0 => {
                    let order = if d[0].abs() < d[2].abs() { [1, 2, 0] } else { [1, 0, 2] };
                    let mut p = from;
                    for a in order {
                        if travel[a] != 0.0 {
                            let mut q = p;
                            q[a] += travel[a];
                            self.inside_between(p, q, &mut seen, w, t);
                            p = q;
                        }
                    }
                }
                _ => self.inside_between(from, to, &mut seen, w, t),
            }
        }
        self.last_moves = moves;
    }

    fn inside_between(&mut self, from: [f64; 3], to: [f64; 3], seen: &mut Vec<[i32; 3]>, w: &dyn World, t: &Tables) {
        let e = f(1.0e-5);
        let shrink = |b: Aabb| Aabb { min: b.min.map(|v| v + e), max: b.max.map(|v| v - e) };
        let (a, b) = (shrink(self.box_at(from)), shrink(self.box_at(to)));
        let lo: [i32; 3] = std::array::from_fn(|i| a.min[i].min(b.min[i]).floor() as i32);
        let hi: [i32; 3] = std::array::from_fn(|i| a.max[i].max(b.max[i]).floor() as i32);
        let far = l2(sub(to, from)) > 0.999_990_000_000_252_6f64.powi(2);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let s = Self::state(w, [x, y, z]);
                    let k = t.inside(s);
                    if k == inside::NONE || k == inside::SLIME || seen.contains(&[x, y, z]) {
                        continue;
                    }
                    seen.push([x, y, z]);
                    let (fx, fy, fz) = (f64::from(x), f64::from(y), f64::from(z));
                    let cell = Aabb { min: [fx, fy, fz], max: [fx + 1.0, fy + 1.0, fz + 1.0] };
                    self.inside_effect(k, [x, y, z], far || b.intersects(&cell), w, t);
                }
            }
        }
    }

    fn inside_effect(&mut self, k: u8, c: [i32; 3], precise: bool, w: &dyn World, t: &Tables) {
        match k {
            inside::WEB => self.stuck = [0.25, f(0.05), 0.25],
            // Non-living entity: always caught in powder snow; berries only
            // slow living ones.
            inside::POWDER_SNOW => self.stuck = [f(0.9), 1.5, f(0.9)],
            inside::HONEY => {
                if let Some(v) = crate::honey_slide(self.pos, self.vel, self.on_ground, self.width as f32, c) {
                    self.vel = v;
                }
            }
            inside::BUBBLE_COLUMN | BUBBLES_DOWN if precise => {
                let down = k == BUBBLES_DOWN;
                let up = Self::state(w, [c[0], c[1] + 1, c[2]]);
                let mut shape = Vec::new();
                t.collision(up, &mut shape);
                let open = shape.is_empty() && t.fluids.fluid(up) == Fluid::None;
                let vy = self.vel[1];
                self.vel[1] = match (open, down) {
                    (true, true) => (vy - 0.03).max(-0.9),
                    (true, false) => (vy + 0.1).min(1.8),
                    (false, true) => (vy - 0.03).max(-0.3),
                    (false, false) => (vy + 0.06).min(0.7),
                };
            }
            _ => {}
        }
    }

    fn friction_below(&self, t: &Tables, w: &dyn World, h: f32) -> f32 {
        let s = Self::state(w, self.below(t, w, h));
        t.friction.get(s as usize).copied().unwrap_or(0.6)
    }

    /// Tick of a dropped item (`tick`: ticks lived, already incremented; `id`: identifier).
    pub fn tick_item(&mut self, w: &dyn World, t: &Tables, tick: i32, id: i32) {
        self.item_tick(w, t, tick, id, None);
    }

    /// Tick of a dropped item on the server side: stuck in a block, it passes through without collision
    /// and frees itself towards the nearest free face (`rand`: entity's own randomness).
    pub fn tick_item_server(&mut self, w: &dyn World, t: &Tables, tick: i32, id: i32, rand: &mut dyn FnMut() -> f32) {
        self.item_tick(w, t, tick, id, Some(rand));
    }

    fn item_tick(&mut self, w: &dyn World, t: &Tables, tick: i32, id: i32, server: Option<&mut dyn FnMut() -> f32>) {
        const G: f64 = 0.04;
        self.old = self.pos;
        self.update_fluids(w, t);
        if self.in_water() && self.water > f(0.1) {
            self.fluid_move(f(0.99));
        } else if self.in_lava() && self.lava > f(0.1) {
            self.fluid_move(f(0.95));
        } else {
            self.apply_gravity(G);
        }
        match server {
            None => self.no_physics = false,
            Some(rand) => {
                self.no_physics = !crate::collide::is_free(w, &self.bounding_box().deflate(1.0e-7));
                if self.no_physics {
                    let b = self.bounding_box();
                    self.towards_closest_space(w, t, [self.pos[0], (b.min[1] + b.max[1]) / 2.0, self.pos[2]], rand);
                }
            }
        }
        let h2 = self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2];
        if self.on_ground && h2 <= f(1.0e-5) && tick.wrapping_add(id) % 4 != 0 {
            self.moved = [0.0; 3];
            self.block_effects(w, t, true);
        } else {
            // The item reads the slowing block almost one block under its feet (0.999999): on
            // a support taller than half a block (soul sand, mud, honey), the block
            // below counts, for the speed factor as for the friction.
            let g = self.gravity(G);
            self.travel(w, t, g, 0.98, 0.999_999);
            self.block_effects(w, t, false);
            let air = 0.98f32;
            let mut ground = air;
            if self.on_ground {
                ground *= self.friction_below(t, w, 0.999_999);
            }
            self.vel = [self.vel[0] * f(ground), self.vel[1] * f(air), self.vel[2] * f(ground)];
            if self.on_ground && self.vel[1] < 0.0 {
                self.vel[1] *= -0.5;
            }
        }
        self.update_fluids(w, t);
    }

    fn fluid_move(&mut self, k: f64) {
        let v = self.vel;
        self.vel = [v[0] * k, v[1] + if v[1] < f(0.06) { f(5.0e-4) } else { 0.0 }, v[2] * k];
    }

    /// Tick of a falling block on the client side: gravity, movement, air friction (placing
    /// and bouncing on the ground are the server's).
    pub fn tick_falling_block(&mut self, w: &dyn World, t: &Tables) {
        self.fall_move(w, t);
        self.vel = self.vel.map(|v| v * f(0.98));
    }

    /// Start of the tick of a falling block: gravity 0.04, movement, block effects (the
    /// server places or bounces the block before the final air friction).
    pub fn fall_move(&mut self, w: &dyn World, t: &Tables) {
        const G: f64 = 0.04;
        self.old = self.pos;
        self.apply_gravity(G);
        let g = self.gravity(G);
        self.travel(w, t, g, 0.98, 0.500_001);
        self.block_effects(w, t, false);
    }

    /// Tick of a primed TNT; `fuse`: fuse remaining, counted down here. Returns false when the
    /// fuse is burnt (the entity also disappears on the client side).
    pub fn tick_tnt(&mut self, w: &dyn World, t: &Tables, fuse: &mut i32) -> bool {
        const G: f64 = 0.04;
        self.old = self.pos;
        self.apply_gravity(G);
        let g = self.gravity(G);
        self.travel(w, t, g, 0.98, 0.500_001);
        self.block_effects(w, t, false);
        self.vel = self.vel.map(|v| v * f(0.98));
        if self.on_ground {
            self.vel = [self.vel[0] * 0.7, self.vel[1] * -0.5, self.vel[2] * 0.7];
        }
        *fuse -= 1;
        if *fuse <= 0 {
            return false;
        }
        self.update_fluids(w, t);
        true
    }

    /// Tick of an experience orb on the client side (the first tick does nothing). `players`:
    /// players known to the client; `rand`: three draws in [0, 1) for the lava.
    pub fn tick_orb(&mut self, w: &dyn World, t: &Tables, players: &[Follow], rand: [f32; 4]) {
        let mut it = rand.into_iter().cycle();
        self.orb_tick(w, t, players, &mut || it.next().unwrap_or(0.0));
    }

    /// Tick of an orb on the server side (no tick skipped; stuck with no tracked player, it
    /// frees itself towards the nearest free face). `rand`: entity's own randomness.
    pub fn tick_orb_server(&mut self, w: &dyn World, t: &Tables, players: &[Follow], rand: &mut dyn FnMut() -> f32) {
        self.server = true;
        self.orb_tick(w, t, players, rand);
    }

    fn orb_tick(&mut self, w: &dyn World, t: &Tables, players: &[Follow], rand: &mut dyn FnMut() -> f32) {
        const G: f64 = 0.03;
        self.moved = [0.0; 3];
        if self.take_first_tick() && !self.server {
            return;
        }
        self.old = self.pos;
        self.update_fluids(w, t);
        let colliding = !crate::collide::is_free(w, &self.bounding_box());
        if self.eyes_in_water {
            let v = self.vel;
            self.vel = [v[0] * f(0.99), (v[1] + f(5.0e-4)).min(f(0.06)), v[2] * f(0.99)];
        } else if !colliding {
            self.apply_gravity(G);
        }
        if t.fluids.fluid(Self::state(w, self.block_pos())) == Fluid::Lava {
            let x = rand() - rand();
            let z = rand() - rand();
            self.vel = [f(x * 0.2), f(0.2), f(z * 0.2)];
        }
        self.follow(players);
        if self.server && self.following.is_none() && colliding {
            let v = self.vel;
            if !crate::collide::is_free(w, &self.bounding_box().offset(v[0], v[1], v[2])) {
                let b = self.bounding_box();
                self.towards_closest_space(w, t, [self.pos[0], (b.min[1] + b.max[1]) / 2.0, self.pos[2]], rand);
            }
        }
        let fall = self.vel[1];
        let g = self.gravity(G);
        self.travel(w, t, g, 0.98, 0.999_999);
        self.block_effects(w, t, false);
        let mut k = 0.98f32;
        if self.on_ground {
            k *= self.friction_below(t, w, 0.999_999);
        }
        self.vel = self.vel.map(|v| v * f(k));
        if self.vertical_below && fall < -g {
            self.vel[1] = -fall * 0.4;
        }
    }

    /// Tick of a free (unhooked) rocket: push (except when fired at an angle), movement with
    /// collisions, speed unchanged by the impact.
    pub fn tick_firework(&mut self, w: &dyn World, t: &Tables, shot_at_angle: bool) {
        self.old = self.pos;
        if !shot_at_angle {
            let h = if self.horizontal_collision { 1.0 } else { 1.15 };
            self.vel = [self.vel[0] * h, self.vel[1] + 0.04, self.vel[2] * h];
        }
        let m = self.vel;
        self.travel(w, t, 0.0, 0.98, 0.500_001);
        self.block_effects(w, t, false);
        self.vel = m;
    }

    /// Water touched by the box (without current push).
    pub fn touching_water(&self, w: &dyn World, t: &Tables) -> bool {
        let mut v = [0.0; 3];
        scan_fluids(w, t, &self.bounding_box(), self.pos, self.pos[1] + self.eye, false, false, &mut v).0 > 0.0
    }

    /// Freeing of a stuck entity (server side): towards the nearest free face (north, south, west, east,
    /// up: neighbour without a full collision shape) from point `p`, at a speed
    /// drawn between 0.1 and 0.3; the other components × 0.75.
    fn towards_closest_space(&mut self, w: &dyn World, _t: &Tables, p: [f64; 3], rand: &mut dyn FnMut() -> f32) {
        let c = p.map(|v| v.floor() as i32);
        let delta: [f64; 3] = std::array::from_fn(|i| p[i] - f64::from(c[i]));
        let mut best = (1usize, 1i32);
        let mut closest = f64::MAX;
        let mut shape = Vec::new();
        for (a, s) in [(2usize, -1i32), (2, 1), (0, -1), (0, 1), (1, 1)] {
            let mut n = c;
            n[a] += s;
            shape.clear();
            w.collision(n[0], n[1], n[2], &mut shape);
            let full = shape.len() == 1 && shape[0].min == [0.0; 3] && shape[0].max == [1.0; 3];
            if !full {
                let o = if s > 0 { 1.0 - delta[a] } else { delta[a] };
                if o < closest {
                    closest = o;
                    best = (a, s);
                }
            }
        }
        let speed = rand() * 0.2 + 0.1;
        let mut v = self.vel.map(|x| x * 0.75);
        v[best.0] = f64::from(best.1 as f32 * speed);
        self.vel = v;
    }

    /// Base of the tick of a projectile or the bobber: fluids touched (kept: the next tick's
    /// inertia uses them) and, if `pushed`, push of the currents on the velocity.
    pub fn base_fluids(&mut self, w: &dyn World, t: &Tables, pushed: bool) {
        let b = self.bounding_box();
        let (water, lava, eyes) = crate::scan_fluids_entity(
            w,
            t,
            &b,
            self.pos,
            self.pos[1] + self.eye,
            pushed,
            self.fast_lava,
            &mut self.vel,
        );
        self.water = water;
        self.lava = lava;
        self.eyes_in_water = eyes;
    }

    /// In water according to the last tick base.
    pub fn was_in_water(&self) -> bool {
        self.in_water()
    }

    /// Effects of the blocks crossed by a direct placement from `from` (projectiles placed without
    /// movement with collisions).
    pub fn effects_after_set(&mut self, w: &dyn World, t: &Tables, from: [f64; 3]) {
        self.old = from;
        self.moves.clear();
        self.block_effects(w, t, false);
    }

    /// First tick of the entity (true only once).
    pub fn take_first_tick(&mut self) -> bool {
        std::mem::replace(&mut self.first_tick, false)
    }

    /// Bubble columns touched by the box, applied as a precise passage (first
    /// tick of thrown items).
    pub fn bubble_columns_inside(&mut self, w: &dyn World, t: &Tables) {
        let b = self.bounding_box();
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.floor() as i32);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let k = t.inside(Self::state(w, [x, y, z]));
                    if k == inside::BUBBLE_COLUMN || k == BUBBLES_DOWN {
                        self.inside_effect(k, [x, y, z], true, w, t);
                    }
                }
            }
        }
    }

    fn follow(&mut self, players: &[Follow]) {
        let d2 = |p: &Follow| (0..3).map(|i| (p.pos[i] - self.pos[i]).powi(2)).sum::<f64>();
        // Tracking resumed if there is none, if it is a spectator or too far (a dying tracked one stays
        // tracked); the nearest within 8 among the non-spectators, rejected if dying.
        let current = self.following.and_then(|id| players.iter().find(|p| p.id == id));
        if current.is_none_or(|p| p.spectator || d2(p) > 64.0) {
            let mut best: Option<(&Follow, f64)> = None;
            for p in players.iter().filter(|p| !p.spectator) {
                let d = d2(p);
                if d < 64.0 && best.is_none_or(|b| d < b.1) {
                    best = Some((p, d));
                }
            }
            self.following = best.filter(|b| b.0.eligible()).map(|b| b.0.id);
        }
        let Some(p) = self.following.and_then(|id| players.iter().find(|p| p.id == id)) else { return };
        let d = [p.pos[0] - self.pos[0], p.pos[1] + p.eye / 2.0 - self.pos[1], p.pos[2] - self.pos[2]];
        let len = l2(d).sqrt();
        let power = 1.0 - len / 8.0;
        let n = if len < f(1.0e-5) { [0.0; 3] } else { d.map(|v| v / len) };
        for (v, n) in self.vel.iter_mut().zip(n) {
            *v += n * (power * power * 0.1);
        }
    }
}

/// First point where the segment `from` → `to` touches the collision shape of a block (walk
/// of the blocks crossed in order, like the game); `None` if it passes freely.
pub fn clip_blocks(w: &dyn World, from: [f64; 3], to: [f64; 3]) -> Option<[f64; 3]> {
    if from == to {
        return None;
    }
    let lerp = |a: [f64; 3], b: [f64; 3], k: f64| -> [f64; 3] { std::array::from_fn(|i| a[i] + k * (b[i] - a[i])) };
    let end = lerp(to, from, -1.0e-7);
    let start = lerp(from, to, -1.0e-7);
    let mut cell = start.map(|v| v.floor() as i32);
    let mut shapes = Vec::new();
    if let Some(p) = clip_cell(w, cell, from, to, &mut shapes) {
        return Some(p);
    }
    let d = sub(end, start);
    let sign = d.map(|v| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let step = std::array::from_fn::<f64, 3, _>(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / d[i] });
    let mut next: [f64; 3] = std::array::from_fn(|i| {
        let frac = start[i] - start[i].floor();
        step[i] * if sign[i] > 0 { 1.0 - frac } else { frac }
    });
    while next[0] <= 1.0 || next[1] <= 1.0 || next[2] <= 1.0 {
        let a = if next[0] < next[1] {
            if next[0] < next[2] { 0 } else { 2 }
        } else if next[1] < next[2] {
            1
        } else {
            2
        };
        cell[a] += sign[a];
        next[a] += step[a];
        if let Some(p) = clip_cell(w, cell, from, to, &mut shapes) {
            return Some(p);
        }
    }
    None
}

fn clip_cell(w: &dyn World, c: [i32; 3], from: [f64; 3], to: [f64; 3], shapes: &mut Vec<Aabb>) -> Option<[f64; 3]> {
    shapes.clear();
    w.collision(c[0], c[1], c[2], shapes);
    if shapes.is_empty() {
        return None;
    }
    let d = sub(to, from);
    if l2(d) < 1.0e-7 {
        return None;
    }
    let o = c.map(f64::from);
    let test: [f64; 3] = std::array::from_fn(|i| from[i] + d[i] * 0.001);
    let inside = |b: &Aabb| (0..3).all(|i| test[i] >= b.min[i] + o[i] && test[i] < b.max[i] + o[i]);
    if shapes.iter().any(inside) {
        return Some(test);
    }
    let mut scale = 1.0f64;
    let mut hit = false;
    for b in shapes.iter() {
        let lo: [f64; 3] = std::array::from_fn(|i| b.min[i] + o[i]);
        let hi: [f64; 3] = std::array::from_fn(|i| b.max[i] + o[i]);
        for a in 0..3 {
            let plane = if d[a] > 1.0e-7 {
                lo[a]
            } else if d[a] < -1.0e-7 {
                hi[a]
            } else {
                continue;
            };
            let s = (plane - from[a]) / d[a];
            let (u, v) = ((a + 1) % 3, (a + 2) % 3);
            let pu = from[u] + s * d[u];
            let pv = from[v] + s * d[v];
            if 0.0 < s
                && s < scale
                && lo[u] - 1.0e-7 < pu
                && pu < hi[u] + 1.0e-7
                && lo[v] - 1.0e-7 < pv
                && pv < hi[v] + 1.0e-7
            {
                scale = s;
                hit = true;
            }
        }
    }
    hit.then(|| std::array::from_fn(|i| from[i] + scale * d[i]))
}

#[cfg(test)]
mod tests;
