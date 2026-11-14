//! Tick of an armor stand or a mannequin, the living entity the game makes of each, in the game's
//! order: the fluids around it (their currents push it, water makes it forget its fall), its fire
//! counting down, the mannequin's wall, breath and countdowns, its small speeds cancelled, its own
//! movement when it has one (in the air, in water or in lava), the blocks it stands on and those it
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
}

impl Body {
    fn read(s: &Server, e: &Nbt, stand: bool) -> Body {
        let (width, height, eye_height) = s.living_size(e, stand);
        Body {
            pos: nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default(),
            vel: nbtio::motion(e.get("Motion")).unwrap_or_default(),
            on_ground: flag(e, "OnGround"),
            fall: fall_distance(e),
            width,
            height,
            eye_height,
            no_gravity: flag(e, "NoGravity"),
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
    /// Speed after the move (the blocked axes stopped).
    vel: [f64; 3],
    /// The movement asked for (stuck: scaled).
    wanted: [f64; 3],
    /// Vertical movement made.
    dy: f64,
    landed: bool,
    horizontal: bool,
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
    /// Tick of armor stand or mannequin `i` of the registry, at `pos`.
    pub(super) fn living_tick(&mut self, i: usize, pos: [f64; 3]) {
        let live = &mut self.deco.live[i];
        live.tick_count += 1;
        let stand = live.kind == DKind::Stand;
        let wet = self.living_fluids(i, stand);
        self.living_fire_tick(i, stand, wet.lava > 0.0);
        if wet.lava > 0.0 {
            self.living_scale_fall(i, 0.5);
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
    /// currents push its velocity, water makes it forget its fall and splashes on entering.
    fn living_fluids(&mut self, i: usize, stand: bool) -> Wet {
        let Some(e) = self.live_nbt(i) else { return Wet::default() };
        let body = Body::read(self, e, stand);
        let mut vel = body.vel;
        let eye = body.pos[1] + body.eye_height;
        let w = self.world();
        let (water, lava, eyes_in_water) =
            rc_physics::scan_fluids_entity(&w, &self.phys, &body.bbox(), body.pos, eye, true, self.fast_lava(), &mut vel);
        let in_water = water > 0.0;
        let live = &mut self.deco.live[i];
        let splash = in_water && !live.was_in_water && live.ticked;
        live.was_in_water = in_water;
        if splash {
            self.living_splash(i, body.vel, body.width);
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
    fn living_splash(&mut self, i: usize, vel: [f64; 3], _width: f64) {
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
    /// (friction of the block under it, gravity). Landing: the block's fall rule, the fall sounds
    /// and damage, the `hit_ground` event.
    fn living_travel(&mut self, i: usize, stand: bool, wet: Wet) -> DecoMove {
        let id = self.deco.live[i].id;
        let Some(e) = self.live_nbt(i) else {
            return DecoMove { index: i, from: [0.0; 3], to: [0.0; 3], wanted: None, size: [0.0; 2], falling: false };
        };
        let body = Body::read(self, e, stand);
        let start = without_small_speeds(body.vel);
        let falling = start[1] <= 0.0;
        let friction = if wet.water > 0.0 || wet.lava > 0.0 || !body.on_ground {
            1.0
        } else {
            let below = [body.pos[0].floor() as i32, (body.pos[1] - 0.500_001).floor() as i32, body.pos[2].floor() as i32];
            self.phys.friction.get(self.st(below) as usize).copied().unwrap_or(0.6)
        };
        let step = self.living_move(i, id, &body, start, stand);
        let mut moved = Body { pos: step.to, vel: step.vel, on_ground: step.landed, ..body };
        // Not in water before the move: the fluids are looked at again where it arrived (their
        // push lands before the drag below, a fall is forgotten in water).
        if wet.water <= 0.0 && self.living_fluids_after_move(i, &mut moved) {
            moved.fall = 0.0;
        }
        let vel = if wet.water > 0.0 {
            self.living_water_drag(id, &moved, falling, step.horizontal)
        } else if wet.lava > 0.0 {
            self.living_lava_drag(id, &moved, wet.lava, falling, step.horizontal)
        } else {
            let fr = f64::from(friction * 0.91f32);
            [moved.vel[0] * fr, (moved.vel[1] - moved.gravity()) * f64::from(0.98f32), moved.vel[2] * fr]
        };
        // The fall of the move: counted outside water (each drop in single precision, like the
        // game's), and on landing its effects below, then forgotten.
        let mut fall = moved.fall;
        if !self.deco.live[i].was_in_water && step.dy < 0.0 {
            fall -= f64::from(step.dy as f32);
        }
        let landing_fall = (step.landed && fall > 0.0).then_some(fall);
        if step.landed {
            fall = 0.0;
        }
        let changed = moved.pos != body.pos || step.landed != body.on_ground || body.vel != vel || fall != body.fall;
        if changed {
            self.deco_edit_at(i, |n| {
                nbtio::set(n, "Pos", dbl(moved.pos));
                nbtio::set(n, "Motion", dbl(vel));
                nbtio::set(n, "OnGround", Nbt::Byte(i8::from(step.landed)));
                nbtio::set(n, "fall_distance", Nbt::Double(fall));
            });
        }
        if let Some(fall) = landing_fall {
            self.living_landed(i, stand, fall);
        }
        let size = [body.width, body.height];
        DecoMove { index: i, from: body.pos, to: moved.pos, wanted: Some(step.wanted), size, falling: fall > 2.5 }
    }

    /// The move itself against the collisions, with the speed `vel` (small speeds cancelled):
    /// stuck in a cobweb or a berry bush by last tick's pass, the movement is scaled and the speed
    /// lost; a blocked axis stops.
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
        if blocked[0] {
            vel[0] = 0.0;
        }
        if blocked[1] {
            vel[2] = 0.0;
        }
        if vertical {
            vel[1] = 0.0;
        }
        Step { to, vel, wanted, dy: mv[1], landed: vertical && wanted[1] < 0.0, horizontal: blocked[0] || blocked[1] }
    }

    /// Fluids looked at again after a move that did not start in water: their push added to the
    /// speed after the move, and the splash on entering water. True in water (its fall forgotten).
    fn living_fluids_after_move(&mut self, i: usize, b: &mut Body) -> bool {
        let mut vel = b.vel;
        let w = self.world();
        let (water, ..) = rc_physics::scan_fluids_entity(
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
        let splash = in_water && !live.was_in_water && live.ticked;
        live.was_in_water = in_water;
        if splash {
            self.living_splash(i, b.vel, b.width);
        }
        b.vel = vel;
        in_water
    }

    /// Water: × 0.8 on every axis (a living entity's slowdown), the sinking adjusted near rest,
    /// and the climb out against a wall.
    fn living_water_drag(&self, id: i32, b: &Body, falling: bool, horizontal: bool) -> [f64; 3] {
        let mut v = b.vel;
        if horizontal && self.phys.climbable(self.st(b.pos.map(|c| c.floor() as i32))) {
            v[1] = 0.2;
        }
        let slow = f64::from(0.8f32);
        let v = Self::fluid_falling(b.gravity(), falling, [v[0] * slow, v[1] * f64::from(0.8f32), v[2] * slow]);
        self.living_jump_out_of_fluid(id, b, v, horizontal)
    }

    /// Lava: shallow (under the jump threshold) like water at half speed, deep halved; a quarter
    /// of the gravity added; the climb out against a wall.
    fn living_lava_drag(&self, id: i32, b: &Body, lava: f64, falling: bool, horizontal: bool) -> [f64; 3] {
        let mut v = if lava <= b.fluid_jump_threshold() {
            Self::fluid_falling(b.gravity(), falling, [b.vel[0] * 0.5, b.vel[1] * f64::from(0.8f32), b.vel[2] * 0.5])
        } else {
            b.vel.map(|c| c * 0.5)
        };
        if b.gravity() != 0.0 {
            v[1] -= b.gravity() / 4.0;
        }
        self.living_jump_out_of_fluid(id, b, v, horizontal)
    }

    /// Against a wall, with room 0.6 above and no liquid there: lifted at 0.3.
    fn living_jump_out_of_fluid(&self, id: i32, b: &Body, v: [f64; 3], horizontal: bool) -> [f64; 3] {
        if !horizontal {
            return v;
        }
        let room = b.bbox().offset(v[0], v[1] + f64::from(0.6f32), v[2]);
        if self.room_free(Self::room_for_object(id, b.pos), &room) && !self.box_has_liquid(&room) {
            return [v[0], f64::from(0.3f32), v[2]];
        }
        v
    }

    /// Landing after a fall of `fall` blocks: the block's fall rule, the fall damage with its
    /// sounds (the armor stand takes none of it but still sounds), the landing dust and the
    /// `hit_ground` event.
    fn living_landed(&mut self, i: usize, stand: bool, fall: f64) {
        let Some(e) = self.live_nbt(i).cloned() else { return };
        let (id, pos) = (self.deco.live[i].id, nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default());
        let ty = if stand { "minecraft:armor_stand" } else { "minecraft:mannequin" };
        let support = self.deco_box(&e).and_then(|b| rc_physics::support_of(&self.world(), &b, pos));
        let on = self.on_pos(pos, support, 0.2);
        let on_state = self.st(on);
        let safe = self.living_attribute(&e, ty, "minecraft:safe_fall_distance", 3.0);
        self.fall_dust(pos, on, fall, safe);
        // Turtle eggs crack before the damage (a living entity that is no player: the rule).
        self.turtle_egg_crush_by_living(on, 3);
        if let Some((dist, mult, dmg_ty)) = self.fall_rule(on_state, fall, false) {
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
        self.game_event_src("minecraft:hit_ground", pos, src, Some(on_state));
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
