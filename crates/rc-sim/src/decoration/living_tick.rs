//! Tick of an armor stand or a mannequin, the living entity the game makes of each, in the game's
//! order: the fluids around it (their currents push it, water makes it forget its fall), its fire
//! counting down, the void's damage under the world, the mannequin's wall, breath and countdowns,
//! its small speeds cancelled, its own movement when it has one (in the air, on a ladder, in water
//! or in lava; landings, bounces, slowing blocks), the blocks it stands on and those it
//! crosses, its freezing; then its network tracking, which runs every tick even without physics.
//!
//! "Physics" follows the game: a marker or weightless armor stand has none (no movement, no block
//! effect); an immovable mannequin keeps its block effects but never moves itself. Without a
//! movement of their own, the currents still add up in their velocity (the game's quirk).

use rc_physics::Aabb;
use rc_proto::nbt::Nbt;

use super::{DKind, dbl, flag, mannequin};
use crate::inside_blocks::{DecoMove, Who};
use crate::nbtio;
use crate::server::Server;

/// Fluids a living decoration touches at the start of its tick (its box, its eyes).
#[derive(Clone, Copy, Default)]
pub(super) struct Wet {
    pub water: f64,
    pub lava: f64,
    pub eyes_in_water: bool,
}

/// What its tick reads of the compound before moving it.
struct Body {
    pos: [f64; 3],
    vel: [f64; 3],
    on_ground: bool,
    fall: f64,
    width: f64,
    height: f64,
    eye_height: f64,
    no_gravity: bool,
    /// Its data or gear may move an attribute: otherwise its movement attributes are the
    /// defaults, answered without reading anything.
    tuned: bool,
}

impl Body {
    fn read(s: &Server, e: &Nbt, stand: bool) -> Body {
        // One walk of the compound tells whether any attribute may differ; then the scale (the
        // size) is looked up only when one may.
        let tuned = super::stand::attributes_may_differ(e);
        let ty = if stand { "minecraft:armor_stand" } else { "minecraft:mannequin" };
        let scale = if tuned { s.living_scale(e, ty) } else { 1.0 };
        let (width, height, eye_height) = super::stand::living_dims(e, stand, scale);
        let (width, height, eye_height) = (f64::from(width), f64::from(height), f64::from(eye_height));
        Body {
            pos: nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default(),
            vel: nbtio::motion(e.get("Motion")).unwrap_or_default(),
            on_ground: flag(e, "OnGround"),
            fall: fall_distance(e),
            width,
            height,
            eye_height,
            no_gravity: flag(e, "NoGravity"),
            tuned,
        }
    }

    fn bbox(&self) -> Aabb {
        let (p, half) = (self.pos, self.width / 2.0);
        Aabb { min: [p[0] - half, p[1], p[2] - half], max: [p[0] + half, p[1] + self.height, p[2] + half] }
    }

    fn gravity(&self) -> f64 {
        if self.no_gravity { 0.0 } else { 0.08 }
    }

    /// Eyes below 0.4: a fluid is deep from its first pixel (the game's jump threshold).
    fn fluid_jump_threshold(&self) -> f64 {
        if self.eye_height < 0.4 { 0.0 } else { 0.4 }
    }
}

/// One move against the collisions.
struct Step {
    to: [f64; 3],
    /// Speed the move started with (zero when stuck), before the collisions change it.
    vel: [f64; 3],
    /// The movement asked for (stuck: scaled).
    wanted: [f64; 3],
    /// Vertical movement made.
    dy: f64,
    /// The vertical movement was cut short (ground or ceiling); `landed`: on the ground.
    vertical: bool,
    landed: bool,
    /// The x and z movements were cut short.
    blocked: [bool; 2],
}

impl Step {
    fn horizontal(&self) -> bool {
        self.blocked[0] || self.blocked[1]
    }
}

/// A landing after a fall: the fall before this move (its dust) and with it (its damage).
#[derive(Clone, Copy)]
struct Landing {
    fall_before_move: f64,
    fall: f64,
}

/// Climbing a ladder, vines or scaffolding: at most 0.15 a tick sideways and downwards.
fn climbing_speed(v: [f64; 3]) -> [f64; 3] {
    let max = f64::from(0.15f32);
    [v[0].clamp(-max, max), v[1].max(-max), v[2].clamp(-max, max)]
}

/// A friction or drag factor moved by its attribute modifier, like the game's.
fn modified_friction(friction: f32, modifier: f32) -> f32 {
    (1.0 - (1.0 - friction) * modifier).clamp(0.0, 1.0)
}

/// Any horizontal speed: friction and speed factors only matter then (most of them rest).
fn slides(v: [f64; 3]) -> bool {
    v[0] != 0.0 || v[2] != 0.0
}

/// Leather boots on its feet: it climbs out of powder snow like the game's walkers.
fn wears_leather_boots(e: &Nbt) -> bool {
    e.get("equipment").and_then(|q| q.get("feet")).and_then(|s| s.get("id")).and_then(Nbt::as_str)
        == Some("minecraft:leather_boots")
}

/// Saved fall distance (0 when absent).
pub(super) fn fall_distance(e: &Nbt) -> f64 {
    match e.get("fall_distance") {
        Some(Nbt::Double(d)) => *d,
        _ => 0.0,
    }
}

/// The armor stand has physics unless it is a marker or weightless; the mannequin always (its
/// weightlessness only removes its gravity).
pub(super) fn has_physics(e: &Nbt, stand: bool) -> bool {
    !stand || !(flag(e, "Marker") || flag(e, "NoGravity"))
}

/// Moves itself: physics, and for a mannequin not immovable.
fn moves_itself(e: &Nbt, stand: bool) -> bool {
    has_physics(e, stand) && (stand || !flag(e, "immovable"))
}

/// Components under 0.003 cancelled, like every living entity's at the start of its movement.
fn without_small_speeds(v: [f64; 3]) -> [f64; 3] {
    v.map(|c| if c.abs() < 0.003 { 0.0 } else { c })
}

impl Server {
    /// Tick of armor stand or mannequin `i` of the registry, at `pos`; `first_tick`: its first
    /// since it entered the world (no splash then, like the game's).
    pub(super) fn living_tick(&mut self, i: usize, pos: [f64; 3], first_tick: bool) {
        let live = &mut self.deco.live[i];
        live.tick_count += 1;
        let stand = live.kind == DKind::Stand;
        let wet = self.living_fluids(i, stand, first_tick);
        self.living_fire_tick(i, stand, wet.lava > 0.0);
        if wet.lava > 0.0 {
            self.living_scale_fall(i, 0.5);
        }
        // Under the world: 4 points every tick, which kills an armor stand at once (the damage
        // bypasses its invulnerability) and hurts a mannequin whenever its hurt cooldown allows.
        if pos[1] < f64::from(self.dim.height.min_y - 64) {
            let id = self.deco.live[i].id;
            self.deco_hurt_by_id(id, stand, "minecraft:out_of_world", 4.0);
            if self.deco.live[i].removed {
                return;
            }
        }
        if !stand {
            self.mannequin_wall_and_breath(i, wet.eyes_in_water);
            if self.mannequin_tick_counts(i) {
                return;
            }
        }
        let Some(e) = self.live_nbt(i) else { return };
        let (physics, moving) = (has_physics(e, stand), moves_itself(e, stand));
        let moved = if moving { self.living_travel(i, stand, wet) } else { self.living_still(i, stand, pos) };
        self.deco.live[i].in_powder_snow = false;
        if physics && !self.deco.live[i].removed {
            self.living_block_effects(i, moved);
        }
        if !self.deco.live[i].removed {
            self.living_freezing(i, stand);
            self.living_track(i, stand);
        }
    }

    /// Fluids touched (box shrunk by 0.001, the eyes' cell), at the start of the tick: the
    /// currents push its velocity, water makes it forget its fall and splashes on entering
    /// (never on its first tick: it was not out of the water before).
    fn living_fluids(&mut self, i: usize, stand: bool, first_tick: bool) -> Wet {
        let Some(e) = self.live_nbt(i) else { return Wet::default() };
        let body = Body::read(self, e, stand);
        let mut vel = body.vel;
        let eye = body.pos[1] + body.eye_height;
        let w = self.world();
        let (water, lava, eyes_in_water) =
            rc_physics::scan_fluids_entity(&w, &self.phys, &body.bbox(), body.pos, eye, true, self.fast_lava(), &mut vel);
        let in_water = water > 0.0;
        let live = &mut self.deco.live[i];
        let splash = in_water && !live.was_in_water && !first_tick;
        live.was_in_water = in_water;
        if splash {
            self.living_splash(i, body.vel);
        }
        let forget_fall = in_water && body.fall != 0.0;
        if vel != body.vel || forget_fall {
            self.deco_edit_at(i, |n| {
                nbtio::set(n, "Motion", dbl(vel));
                if forget_fall {
                    nbtio::set(n, "fall_distance", Nbt::Double(0.0));
                }
            });
        }
        Wet { water, lava, eyes_in_water }
    }

    /// Entering water: splash sound (volume from the speed before the push), `splash` event.
    fn living_splash(&mut self, i: usize, vel: [f64; 3]) {
        let Some(e) = self.live_nbt(i).cloned() else { return };
        let speed = crate::mobs::aquatic::splash::sound_speed(vel, 0.2);
        let mut r = super::decoration_sound_rng();
        let pitch = 1.0 + (r.next_float() - r.next_float()) * 0.4;
        self.deco_sound(&e, "minecraft:entity.generic.splash", 6, speed, pitch);
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.game_event_src("minecraft:splash", pos, Some(crate::mobs::recent::vibration::Src::Other), None);
    }

    /// Fall distance multiplied by `k` (in lava: halved each tick).
    fn living_scale_fall(&mut self, i: usize, k: f64) {
        let Some(e) = self.live_nbt(i) else { return };
        let fall = fall_distance(e);
        if fall != 0.0 {
            self.deco_edit_at(i, |n| nbtio::set(n, "fall_distance", Nbt::Double(fall * k)));
        }
    }

    /// No movement of its own: only its small speeds cancelled (the currents' push stays and
    /// adds up, as in the game). Its movement for the block pass: none.
    fn living_still(&mut self, i: usize, stand: bool, pos: [f64; 3]) -> DecoMove {
        let mut still = DecoMove { index: i, from: pos, to: pos, wanted: None, size: [0.0; 2], falling: false };
        if let Some(e) = self.live_nbt(i) {
            let (w, h, _) = self.living_size(e, stand);
            (still.size, still.falling) = ([w, h], fall_distance(e) > 2.5);
            let saved = nbtio::motion(e.get("Motion")).unwrap_or_default();
            let v = without_small_speeds(saved);
            // Compared bit for bit: a saved -0.0 becomes 0.0 like the game's.
            if v.map(f64::to_bits) != saved.map(f64::to_bits) {
                self.deco_edit_at(i, |n| nbtio::set(n, "Motion", dbl(v)));
            }
        }
        still
    }

    /// Its own movement: in water or lava the fluid's drag and sinking, otherwise the air's
    /// (climbing held to 0.15, friction of the block under it, gravity). The move itself runs the
    /// game's fall check (the fluids where it arrived, the landing), the restitution after a
    /// collision (the bounces) and the speed factor of the block it is in or on.
    fn living_travel(&mut self, i: usize, stand: bool, wet: Wet) -> DecoMove {
        let id = self.deco.live[i].id;
        let Some(e) = self.live_nbt(i) else {
            return DecoMove { index: i, from: [0.0; 3], to: [0.0; 3], wanted: None, size: [0.0; 2], falling: false };
        };
        let body = Body::read(self, e, stand);
        let in_fluid = wet.water > 0.0 || wet.lava > 0.0;
        let mut start = without_small_speeds(body.vel);
        let falling = start[1] <= 0.0;
        let mut fall = body.fall;
        // A ladder only changes something for a fall to forget or a speed beyond its limits: the
        // block is not read for the many that rest.
        let beyond = start != climbing_speed(start) || fall != 0.0;
        if !in_fluid && beyond && self.living_on_climbable(body.pos) {
            fall = 0.0;
            start = climbing_speed(start);
        }
        // The friction is the one under it before the move; read only for a slide.
        let friction = (!in_fluid && body.on_ground && slides(start)).then(|| self.living_block_friction(i, &body));
        let step = self.living_move(i, id, &body, start, stand);
        let mut moved = Body { pos: step.to, vel: step.vel, on_ground: step.landed, fall, ..body };
        let (landing, lava) = self.living_fall_check(i, &step, &mut moved, wet);
        let mut support = None;
        let bounced = self.living_after_collision(i, &step, &mut moved, &mut support);
        // In lava from the start of its tick: the drag of the lava's height where it arrived.
        let vel = if wet.water > 0.0 {
            self.living_water_drag(id, &moved, falling, step.horizontal(), body.pos[1])
        } else if wet.lava > 0.0 {
            self.living_lava_drag(id, &moved, lava, falling, step.horizontal(), body.pos[1])
        } else {
            // Against a wall: it climbs a ladder, or out of powder snow it was in at its last
            // pass when its boots let it walk on it.
            let climbs_out_of_snow =
                || self.deco.live[i].in_powder_snow && self.live_nbt(i).is_some_and(wears_leather_boots);
            if step.horizontal() && (self.living_on_climbable(moved.pos) || climbs_out_of_snow()) {
                moved.vel[1] = 0.2;
            }
            let friction = match friction {
                Some(f) => f,
                None if body.on_ground && slides(moved.vel) => self.living_block_friction(i, &body),
                None => 1.0,
            };
            self.living_air_drag(i, &moved, friction)
        };
        let changed = moved.pos != body.pos || step.landed != body.on_ground || body.vel != vel || moved.fall != body.fall;
        if changed {
            self.deco_edit_at(i, |n| {
                nbtio::set(n, "Pos", dbl(moved.pos));
                nbtio::set(n, "Motion", dbl(vel));
                nbtio::set(n, "OnGround", Nbt::Byte(i8::from(step.landed)));
                nbtio::set(n, "fall_distance", Nbt::Double(moved.fall));
            });
        }
        if let Some(landing) = landing {
            let support = *support.get_or_insert_with(|| self.living_support(&moved));
            self.living_landed(i, stand, landing, support);
        }
        if bounced {
            self.living_bounced(&moved, &mut support);
        }
        let size = [body.width, body.height];
        DecoMove { index: i, from: body.pos, to: moved.pos, wanted: Some(step.wanted), size, falling: moved.fall > 2.5 }
    }

    /// In a climbable block (ladder, vines, scaffolding: the `climbable` tag).
    fn living_on_climbable(&self, pos: [f64; 3]) -> bool {
        self.phys.climbable(self.st(pos.map(|c| c.floor() as i32)))
    }

    /// Its main supporting block when on the ground (searched only when asked).
    fn living_support(&self, b: &Body) -> Option<[i32; 3]> {
        if !b.on_ground {
            return None;
        }
        rc_physics::support_of(&self.world(), &b.bbox(), b.pos)
    }

    /// A movement attribute: the default unless its data or gear may move it.
    fn living_movement_attribute(&self, i: usize, b: &Body, name: &str, default: f64) -> f64 {
        if !b.tuned {
            return default;
        }
        let ty = self.deco.live[i].kind.name();
        self.live_nbt(i).and_then(|e| self.nbt_attribute(e, ty, name)).unwrap_or(default)
    }

    /// Friction of the block under it that affects its movement (0.500001 under its feet, from
    /// its supporting block), moved by its friction modifier.
    fn living_block_friction(&self, i: usize, b: &Body) -> f32 {
        let below = self.on_pos(b.pos, self.living_support(b), 0.500_001);
        let block = self.phys.friction.get(self.st(below) as usize).copied().unwrap_or(0.6);
        modified_friction(block, self.living_movement_attribute(i, b, "minecraft:friction_modifier", 1.0) as f32)
    }

    /// The air's drag after its move: friction times the air's 0.91 sideways, gravity then 0.98
    /// vertically (both moved by its air drag modifier).
    fn living_air_drag(&self, i: usize, b: &Body, friction: f32) -> [f64; 3] {
        let modifier = self.living_movement_attribute(i, b, "minecraft:air_drag_modifier", 1.0) as f32;
        let sideways = f64::from(friction * modified_friction(0.91, modifier));
        let vertical = f64::from(modified_friction(0.98, modifier));
        [b.vel[0] * sideways, (b.vel[1] - b.gravity()) * vertical, b.vel[2] * sideways]
    }

    /// The move itself against the collisions, with the speed `vel` (small speeds cancelled):
    /// stuck in a cobweb or a berry bush by last tick's pass, the movement is scaled and the speed
    /// lost.
    fn living_move(&mut self, i: usize, id: i32, body: &Body, mut vel: [f64; 3], stand: bool) -> Step {
        let stuck = std::mem::take(&mut self.deco.live[i].stuck);
        let mut wanted = vel;
        if stuck.iter().map(|c| c * c).sum::<f64>() > 1.0e-7 {
            wanted = [vel[0] * stuck[0], vel[1] * stuck[1], vel[2] * stuck[2]];
            vel = [0.0; 3];
        }
        let step = if stand { 0.0 } else { f64::from(0.6f32) };
        let world = self.world_for_room(Self::room_for_object(id, body.pos));
        let mv = rc_physics::collide::resolve(&world, &body.bbox(), wanted, body.on_ground, step);
        let l2 = |v: [f64; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
        let mut to = body.pos;
        if l2(mv) > 1.0e-7 || l2(wanted) - l2(mv) < 1.0e-7 {
            to = [body.pos[0] + mv[0], body.pos[1] + mv[1], body.pos[2] + mv[2]];
        }
        let vertical = wanted[1] != mv[1];
        let blocked = [(wanted[0] - mv[0]).abs() >= 1.0e-5, (wanted[2] - mv[2]).abs() >= 1.0e-5];
        Step { to, vel, wanted, dy: mv[1], vertical, landed: vertical && wanted[1] < 0.0, blocked }
    }

    /// The fall check of the move: outside water before it, the fluids looked at again where it
    /// arrived (their push lands on the speed it moved with, water makes it forget its fall); the
    /// fall then counted outside water (each drop in single precision, like the game's), and
    /// forgotten on landing. The landing, when it fell, and the lava's height it is in now.
    fn living_fall_check(&mut self, i: usize, step: &Step, moved: &mut Body, wet: Wet) -> (Option<Landing>, f64) {
        let mut lava = wet.lava;
        if wet.water <= 0.0 {
            let (in_water, lava_now) = self.living_fluids_after_move(i, moved);
            lava = lava_now;
            if in_water {
                moved.fall = 0.0;
            }
        }
        let fall_before_move = moved.fall;
        if !self.deco.live[i].was_in_water && step.dy < 0.0 {
            moved.fall -= f64::from(step.dy as f32);
        }
        let landing = (step.landed && moved.fall > 0.0).then_some(Landing { fall_before_move, fall: moved.fall });
        if step.landed {
            moved.fall = 0.0;
        }
        (landing, lava)
    }

    /// After a collision, the game's restitution then the block's speed factor. A blocked axis
    /// bounces back by its own bounciness (none by default: it stops, its zero signed like the
    /// game's `-v × 0`). Landing faster than its gravity on a block that bounces (slime, beds)
    /// and does not suppress it, it bounces up, the part of the tick it fell compensated for
    /// gravity and drag. True when it bounced. `support`: its supporting block, once searched.
    fn living_after_collision(
        &self,
        i: usize,
        step: &Step,
        b: &mut Body,
        support: &mut Option<Option<[i32; 3]>>,
    ) -> bool {
        let mut bounced = false;
        if step.vertical || step.horizontal() {
            let own = self.living_movement_attribute(i, b, "minecraft:bounciness", 0.0);
            let cur = b.vel;
            if step.blocked[0] {
                b.vel[0] = -cur[0] * own;
            }
            if step.blocked[1] {
                b.vel[2] = -cur[2] * own;
            }
            bounced = own > 0.0 && step.horizontal();
            if step.vertical {
                let mut restitution = own;
                // Resting ones land each tick slower than their gravity: no block read for them.
                if step.landed {
                    restitution = if -cur[1] > b.gravity() { self.living_block_bounce(b, own, support) } else { 0.0 };
                }
                let (compensation, drag) = if restitution > 0.0 {
                    bounced = true;
                    let portion = step.dy / cur[1];
                    let air = self.living_movement_attribute(i, b, "minecraft:air_drag_modifier", 1.0) as f32;
                    (portion * b.gravity(), 1.0 + portion * (f64::from(modified_friction(0.98, air)) - 1.0))
                } else {
                    (0.0, 1.0)
                };
                b.vel[1] = (compensation - cur[1]) * drag * restitution;
            }
        }
        if slides(b.vel) {
            let factor = f64::from(self.living_speed_factor(i, b, support));
            (b.vel[0], b.vel[2]) = (b.vel[0] * factor, b.vel[2] * factor);
        }
        bounced
    }

    /// The bounce of the block 0.2 under its feet (its supporting block first), at least `own`;
    /// none on a block of `#suppresses_bounce`.
    fn living_block_bounce(&self, b: &Body, own: f64, support: &mut Option<Option<[i32; 3]>>) -> f64 {
        let s = *support.get_or_insert_with(|| self.living_support(b));
        let state = self.st(self.on_pos(b.pos, s, 0.2));
        if self.block_has_tag(state, crate::tag_bits::TagId::SuppressesBounce) {
            return 0.0;
        }
        own.max(f64::from(self.phys.bounce.get(state as usize).copied().unwrap_or(0.0)))
    }

    /// Speed factor of the block it is in (water and bubble columns: final), otherwise of the
    /// block under it (soul sand, honey), brought towards 1 by its movement efficiency.
    fn living_speed_factor(&self, i: usize, b: &Body, support: &mut Option<Option<[i32; 3]>>) -> f32 {
        let factor_of = |s: u32| self.phys.speed_factor.get(s as usize).copied().unwrap_or(1.0);
        let here = self.st(b.pos.map(|c| c.floor() as i32));
        let mut factor = factor_of(here);
        if factor == 1.0 && !self.phys.speed_factor_here_only(here) {
            let s = *support.get_or_insert_with(|| self.living_support(b));
            factor = factor_of(self.st(self.on_pos(b.pos, s, 0.500_001)));
        }
        let efficiency = self.living_movement_attribute(i, b, "minecraft:movement_efficiency", 0.0) as f32;
        factor + efficiency * (1.0 - factor)
    }

    /// A bounce off the block: the shelf mushroom's sound (at the block under it that affects
    /// its movement), the `bounce` event.
    fn living_bounced(&mut self, b: &Body, support: &mut Option<Option<[i32; 3]>>) {
        let s = *support.get_or_insert_with(|| self.living_support(b));
        let effect = self.st(self.on_pos(b.pos, s, 0.2));
        if self.data.block_of(effect).is_some_and(|k| k.name == "minecraft:shelf_mushroom") {
            let below = self.on_pos(b.pos, s, 0.500_001);
            self.block_sound(below, "minecraft:block.shelf_mushroom.bounce", 1.0, 1.0);
        }
        self.game_event_src("minecraft:bounce", b.pos, Some(crate::mobs::recent::vibration::Src::Other), None);
    }

    /// Fluids looked at again after a move that did not start in water: their push added to the
    /// speed it moved with, and the splash on entering water (its first tick is over by then).
    /// In water now (its fall forgotten), and the height of the lava it is in.
    fn living_fluids_after_move(&mut self, i: usize, b: &mut Body) -> (bool, f64) {
        let mut vel = b.vel;
        let w = self.world();
        let (water, lava, _) = rc_physics::scan_fluids_entity(
            &w,
            &self.phys,
            &b.bbox(),
            b.pos,
            b.pos[1] + b.eye_height,
            true,
            self.fast_lava(),
            &mut vel,
        );
        let in_water = water > 0.0;
        let live = &mut self.deco.live[i];
        let splash = in_water && !live.was_in_water;
        live.was_in_water = in_water;
        if splash {
            self.living_splash(i, b.vel);
        }
        b.vel = vel;
        (in_water, lava)
    }

    /// Water: × 0.8 on every axis (a living entity's slowdown), the sinking adjusted near rest,
    /// and the climb out against a wall.
    fn living_water_drag(&self, id: i32, b: &Body, falling: bool, horizontal: bool, old_y: f64) -> [f64; 3] {
        let mut v = b.vel;
        if horizontal && self.living_on_climbable(b.pos) {
            v[1] = 0.2;
        }
        let slow = f64::from(0.8f32);
        let v = Self::fluid_falling(b.gravity(), falling, [v[0] * slow, v[1] * f64::from(0.8f32), v[2] * slow]);
        self.living_jump_out_of_fluid(id, b, v, horizontal, old_y)
    }

    /// Lava: shallow (under the jump threshold) like water at half speed, deep halved; a quarter
    /// of the gravity added; the climb out against a wall.
    fn living_lava_drag(&self, id: i32, b: &Body, lava: f64, falling: bool, horizontal: bool, old_y: f64) -> [f64; 3] {
        let mut v = if lava <= b.fluid_jump_threshold() {
            Self::fluid_falling(b.gravity(), falling, [b.vel[0] * 0.5, b.vel[1] * f64::from(0.8f32), b.vel[2] * 0.5])
        } else {
            b.vel.map(|c| c * 0.5)
        };
        if b.gravity() != 0.0 {
            v[1] -= b.gravity() / 4.0;
        }
        self.living_jump_out_of_fluid(id, b, v, horizontal, old_y)
    }

    /// Against a wall, with room and no liquid where its box would be once lifted 0.6 above the
    /// height it started the move at (`old_y`), carried by its new speed: lifted at 0.3.
    fn living_jump_out_of_fluid(&self, id: i32, b: &Body, v: [f64; 3], horizontal: bool, old_y: f64) -> [f64; 3] {
        if !horizontal {
            return v;
        }
        let room = b.bbox().offset(v[0], v[1] + f64::from(0.6f32) - b.pos[1] + old_y, v[2]);
        if self.room_free(Self::room_for_object(id, b.pos), &room) && !self.box_has_liquid(&room) {
            return [v[0], f64::from(0.3f32), v[2]];
        }
        v
    }

    /// Landing after a fall: the landing dust (on the fall before this move), the block's fall
    /// rule, the fall damage with its sounds (the armor stand takes none of it but still sounds)
    /// and the `hit_ground` event (the supporting block's state).
    fn living_landed(&mut self, i: usize, stand: bool, landing: Landing, support: Option<[i32; 3]>) {
        let Some(e) = self.live_nbt(i).cloned() else { return };
        let (id, pos) = (self.deco.live[i].id, nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default());
        let ty = if stand { "minecraft:armor_stand" } else { "minecraft:mannequin" };
        let on = self.on_pos(pos, support, 0.2);
        let on_state = self.st(on);
        let safe = self.living_attribute(&e, ty, "minecraft:safe_fall_distance", 3.0);
        if landing.fall_before_move > 0.0 {
            self.fall_dust(pos, on, landing.fall_before_move, safe);
        }
        // Turtle eggs crack before the damage (a living entity that is no player: the rule).
        self.turtle_egg_crush_by_living(on, 3);
        if let Some((dist, mult, dmg_ty)) = self.fall_rule(on_state, landing.fall, false) {
            let factor = self.living_attribute(&e, ty, "minecraft:fall_damage_multiplier", 1.0);
            let dmg = ((dist + 1.0e-6 - safe) * mult * factor).floor() as i32;
            if dmg > 0 {
                let sound = match (stand, dmg > 4) {
                    (true, _) => "minecraft:entity.armor_stand.fall",
                    (false, true) => "minecraft:entity.generic.big_fall",
                    (false, false) => "minecraft:entity.generic.small_fall",
                };
                self.deco_sound(&e, sound, 6, 1.0, 1.0);
                if let Some((s, v, p)) = self.block_fall_sound_at(pos) {
                    self.deco_sound(&e, &s, 6, v, p);
                }
                self.deco_hurt_by_id(id, stand, dmg_ty, dmg as f32);
            }
        }
        let src = Some(crate::mobs::recent::vibration::Src::Other);
        let ground = self.st(support.unwrap_or(on));
        self.game_event_src("minecraft:hit_ground", pos, src, Some(ground));
    }

    /// An attribute of a living decoration: its type's default unless its data or gear can move
    /// it (the usual case answered without building anything).
    pub(super) fn living_attribute(&self, e: &Nbt, ty: &str, name: &str, default: f64) -> f64 {
        if !super::stand::attribute_may_differ(e, name) {
            return default;
        }
        self.nbt_attribute(e, ty, name).unwrap_or(default)
    }

    /// The blocks it stands on (on the ground) then those its movement crossed (fire, lava,
    /// cactus, campfire, powder snow, cauldrons, honey, bubble columns, plates, tripwire, berry
    /// bushes, cobwebs).
    fn living_block_effects(&mut self, i: usize, moved: DecoMove) {
        if self.live_nbt(i).is_some_and(|e| flag(e, "OnGround")) {
            self.deco_step_on(i);
            if self.deco.live[i].removed {
                return;
            }
        }
        // Most of them stand among blocks the pass does nothing with: then only their contacts
        // (plates, tripwire, berry bushes, cobwebs), which the pass would play the same way.
        let fire = self.deco_fire_at(i);
        if self.deco.switches.quiet_block_pass && fire == 0 && self.living_pass_is_quiet(&moved) {
            self.deco_contacts(&moved);
            return;
        }
        let out = self.inside_blocks(&mut Who::Deco(moved));
        if let Some(l) = self.deco.live.get_mut(i) {
            l.in_powder_snow = out.in_powder_snow;
        }
    }

    /// No cell its box covers, where it was and where it is, holds a block or a fluid the pass acts
    /// with for something that does not burn (water only puts a fire out, the other blocks only
    /// split the steps).
    fn living_pass_is_quiet(&self, moved: &DecoMove) -> bool {
        let b = moved.bbox();
        let back = b.offset(moved.from[0] - moved.to[0], moved.from[1] - moved.to[1], moved.from[2] - moved.to[2]);
        let both = b.union(&back).deflate(f64::from(1.0e-5f32));
        self.no_living_pass_block_in(&both)
    }

    /// The supporting block is looked for only when a block under its feet (at the step height,
    /// or one lower for a fence or wall it stands on) may react to a step.
    pub(super) fn living_step_may_act(&self, e: &Nbt, pos: [f64; 3]) -> bool {
        if !self.deco.switches.step_on_shortcut {
            return true;
        }
        let Some(b) = self.deco_box(e) else { return true };
        let y = (pos[1] - f64::from(0.2f32)).floor() as i32;
        let (x0, x1, z0, z1) = (b.min[0].floor() as i32, b.max[0].floor() as i32, b.min[2].floor() as i32, b.max[2].floor() as i32);
        let ids = &self.bs.t.id;
        let vib = &self.mobs.rec.vib_blocks;
        (y - 1..=y).any(|y| {
            (x0..=x1).any(|x| {
                (z0..=z1).any(|z| {
                    let block = self.view().block(self.st([x, y, z]));
                    block == ids.magma
                        || block == ids.slime
                        || block == ids.redstone_ore
                        || block == ids.deepslate_redstone_ore
                        || block == ids.turtle_egg
                        || vib.contains_key(&[x, y, z])
                })
            })
        })
    }

    /// Freezing after the pass: −2 outside powder snow (or when it cannot freeze); a frozen
    /// mannequin takes 1 freezing damage every 40 ticks.
    fn living_freezing(&mut self, i: usize, stand: bool) {
        let frozen = self.deco_frozen_at(i);
        // Not freezing, the usual case: nothing thaws, nothing hurts.
        if frozen == 0 {
            return;
        }
        let can = self.deco_can_freeze_at(i);
        if (!self.deco.live[i].in_powder_snow || !can) && frozen > 0 {
            self.deco_set_frozen_at(i, (frozen - 2).max(0));
        }
        let fully = self.deco_frozen_at(i) >= crate::inside_blocks::FREEZE_REQUIRED;
        if !stand && can && fully && self.deco.live[i].tick_count % 40 == 0 {
            let id = self.deco.live[i].id;
            self.mannequin_hurt(id, "minecraft:freeze", 1.0, None, None);
        }
    }

    /// Network tracking of its tick (position, velocity, ground, rotation).
    fn living_track(&mut self, i: usize, stand: bool) {
        let Some(e) = self.live_nbt(i) else { return };
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let vel = nbtio::motion(e.get("Motion")).unwrap_or_default();
        let on_ground = flag(e, "OnGround");
        let rot = nbtio::f32s::<2>(e.get("Rotation")).unwrap_or_default();
        let interval = if stand { 3 } else { 2 };
        let id = self.deco.live[i].id;
        let mut t = std::mem::take(&mut self.deco.live[i].track);
        self.track_turned(&mut t, id, pos, vel, on_ground, interval, rot);
        self.deco.live[i].track = t;
    }

    /// Hurt of a living decoration by its identifier (`stand`: an armor stand's rules).
    pub(super) fn deco_hurt_by_id(&mut self, id: i32, stand: bool, ty: &str, amount: f32) -> bool {
        if stand { self.stand_hurt_by_world(id, ty, amount) } else { self.mannequin_hurt(id, ty, amount, None, None) }
    }

    /// The fire at the start of its tick, as any living entity's: a burning counter hurts every
    /// 20 ticks (`on_fire`, never while in lava) and loses one tick; the clients are told when it
    /// starts or stops being shown burning.
    pub(super) fn living_fire_tick(&mut self, i: usize, stand: bool, in_lava: bool) {
        let id = self.deco.live[i].id;
        let Some(e) = self.live_nbt(i) else { return };
        let before = mannequin::fire_ticks(e);
        // At rest and shown so, the usual case: one field read, nothing changes.
        if before <= 0 && !self.deco.live[i].shown_burning {
            return;
        }
        if before > 0 && before % 20 == 0 && !in_lava {
            self.deco_hurt_by_id(id, stand, "minecraft:on_fire", 1.0);
            if self.deco.live[i].removed {
                return;
            }
        }
        // Read again: the hurt may have changed it; the tick counted off what is there now.
        let Some(e) = self.live_nbt(i) else { return };
        let now = mannequin::fire_ticks(e);
        let fire = if before > 0 { now - 1 } else { now };
        let (shown, flags) = (mannequin::shown_burning(e, fire), crate::drops::shared_flags(e, fire > 0));
        if shown != self.deco.live[i].shown_burning {
            self.deco.live[i].shown_burning = shown;
            self.deco_data(id, vec![(0, rc_proto_v777::entity::Meta::Byte(flags))]);
        }
        if fire != now {
            self.deco_edit_at(i, |n| nbtio::set(n, "Fire", Nbt::Short(fire as i16)));
        }
    }
}
