//! Mob physics (`server-mobs.md` § 1): fluids, movement (`travel`) in
//! air, water and lava, collisions and step-up, jump, fall, mutual push.

use rc_physics::collide;

use super::env::Fluid;
use super::mob::{Ent, Mob};
use super::mth::{DEG, cos, sin};
use super::species::Kind;
use crate::server::Server;

/// `RUSTCRAFT_OPT_PUSHIDX=0`: each push finds its mob by `mob_index` (old
/// path) instead of the recorded slot.
fn push_idx() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_PUSHIDX") != Some("0"))
}

fn l2(v: [f64; 3]) -> f64 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// Powder snow as seen by a mob: low shape (0.9) after a fall of more than 2.5 blocks;
/// otherwise full block for a species in `#powder_snow_walkable_mobs` (or wearing leather
/// boots) whose bottom is above the block; empty for the others.
struct SnowStand<'a> {
    inner: &'a dyn rc_physics::World,
    s: &'a Server,
    bottom: f64,
    falling: bool,
    walker: bool,
}

impl rc_physics::World for SnowStand<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.inner.state(x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        let over = self.inner.collision(x, y, z, out);
        if out.is_empty() && self.s.bname([x, y, z]) == "minecraft:powder_snow" {
            if self.falling {
                out.push(rc_physics::Aabb { min: [0.0; 3], max: [1.0, f64::from(0.9f32), 1.0] });
            } else if self.walker && self.bottom > f64::from(y) + 1.0 - f64::from(1.0e-5f32) {
                out.push(rc_physics::Aabb { min: [0.0; 3], max: [1.0; 3] });
            }
        }
        over
    }

    fn border(&self) -> Option<&rc_physics::WorldBorder> {
        self.inner.border()
    }

    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        self.inner.moving_collision(x, y, z, out)
    }
}

fn corrected(x: f32, m: f32) -> f32 {
    (1.0 - (1.0 - x) * m).clamp(0.0, 1.0)
}

impl Server {
    /// Fluids touched and current, start of the base tick.
    pub(crate) fn mob_fluids(&self, m: &mut Mob) {
        let w = self.world();
        let eye = m.eye_y();
        let mut flow = m.vel;
        let (water, lava, eyes) =
            rc_physics::scan_fluids_entity(&w, &self.phys, &m.bbox(), m.pos, eye, true, self.fast_lava(), &mut flow);
        // F09: aquatic fauna and nautiluses are not pushed by currents.
        if super::surface::pushed_by_fluid(m.kind)
            && super::aquatic::pushed_by_fluid(m)
            && !super::sealife::handles(m.kind)
            && super::brainy::pushed_by_fluid(m.kind)
        {
            m.vel = flow;
        }
        m.water_height = water;
        m.lava_height = lava;
        m.in_water = water > 0.0;
        m.eyes_in_water = eyes;
        if m.in_water {
            m.fall_distance = 0.0;
            if m.fire > 0 {
                m.fire = 0;
            }
        }
    }

    /// Block under the feet that acts on movement.
    pub(crate) fn mob_below(&self, m: &Mob, off: f32) -> [i32; 3] {
        self.on_pos(m.pos, m.support, off)
    }

    /// Block under an entity at `off` below its feet (single-precision offset, like the game):
    /// main supporting block at 1e-5, otherwise its column at that height, except low walls, fence gates
    /// and (offset <= 0.5) fences that keep the supporter; without a supporter, the cell under `pos`.
    /// Single rule shared by mobs, player, minecarts and predicates.
    pub(crate) fn on_pos(&self, pos: [f64; 3], support: Option<[i32; 3]>, off: f32) -> [i32; 3] {
        let y = (pos[1] - f64::from(off)).floor() as i32;
        if let Some(s) = support {
            if off <= 1.0e-5 {
                return s;
            }
            let fence = self.btag_id(s, crate::tag_bits::TagId::Fences);
            let keep = (f64::from(off) <= 0.5 && fence)
                || self.btag_id(s, crate::tag_bits::TagId::Walls)
                || self.bname(s).ends_with("_fence_gate");
            return if keep { s } else { [s[0], y, s[2]] };
        }
        [pos[0].floor() as i32, y, pos[2].floor() as i32]
    }

    fn friction_at(&self, p: [i32; 3]) -> f32 {
        self.phys.friction.get(self.st(p) as usize).copied().unwrap_or(0.6)
    }

    fn speed_factor(&self, m: &Mob) -> f32 {
        let here = m.block_pos();
        let s = self.st(here);
        let f = self.phys.speed_factor.get(s as usize).copied().unwrap_or(1.0);
        let name = self.state_name(s);
        if name == "minecraft:water" || name == "minecraft:bubble_column" || f != 1.0 {
            return f;
        }
        let b = self.st(self.mob_below(m, 0.500_001));
        self.phys.speed_factor.get(b as usize).copied().unwrap_or(1.0)
    }

    fn jump_factor(&self, m: &Mob) -> f32 {
        let here = self.phys.jump_factor.get(self.st(m.block_pos()) as usize).copied().unwrap_or(1.0);
        let below = self.phys.jump_factor.get(self.st(self.mob_below(m, 0.500_001)) as usize).copied().unwrap_or(1.0);
        if here == 1.0 { below } else { here }
    }

    pub(crate) fn on_climbable(&self, m: &Mob) -> bool {
        if m.kind.base() == Kind::Spider {
            return m.climbing;
        }
        self.phys.climbable(self.st(m.block_pos()))
    }

    pub(crate) fn jump_from_ground(&mut self, m: &mut Mob) {
        // Cubes: imposed vertical velocity (no threshold or maximum), magma: + size × 0.1.
        if m.kind.is_cube() {
            let mut p = m.attrs.jump.value() as f32 * self.jump_factor(m);
            if m.kind == Kind::MagmaCube {
                p += m.cube.size as f32 * 0.1;
            }
            m.vel[1] = f64::from(p);
            m.needs_sync = true;
            return;
        }
        let p = m.attrs.jump.value() as f32 * super::surface::jump_mult(self, m) * self.jump_factor(m);
        if p <= 1.0e-5 {
            return;
        }
        m.vel[1] = f64::from(p).max(m.vel[1]);
        super::surface::after_jump(self, m);
        super::brainy::after_jump(m); // F10: sniffer
        m.needs_sync = true;
    }

    /// Acceleration relative to the yaw.
    fn move_relative(m: &mut Mob, speed: f32) {
        let input = [f64::from(m.xxa), f64::from(m.yya), f64::from(m.zza)];
        let len = l2(input);
        if len < 1.0e-7 {
            return;
        }
        let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
        let v = v.map(|x| x * f64::from(speed));
        let (s, c) = (sin(m.yaw * DEG), cos(m.yaw * DEG));
        m.vel[0] += v[0] * f64::from(c) - v[2] * f64::from(s);
        m.vel[1] += v[1];
        m.vel[2] += v[2] * f64::from(c) + v[0] * f64::from(s);
    }

    /// Collisions, flags, fall, step, speed factor.
    pub(crate) fn mob_move(&mut self, m: &mut Mob, delta: [f64; 3]) {
        let start = m.bbox();
        // F12: the strider settles on lava sources (own shape above them).
        let mv = {
            let w = self.world();
            let lava = super::nether::strider::lava_stand(self, m, start.min[1]);
            let wd: &dyn rc_physics::World = match &lava {
                Some(l) => l,
                None => &w,
            };
            // Powder snow: shape specific to the entity (fall, species that walk on it).
            let falling = m.fall_distance > 2.5;
            let walker = m.kind.powder_snow_walkable()
                || m.equipment[2].as_ref().is_some_and(|s| s.id == "minecraft:leather_boots");
            if falling || walker {
                let snow = SnowStand { inner: wd, s: self, bottom: start.min[1], falling, walker };
                collide::resolve(&snow, &start, delta, m.on_ground, m.step_height())
            } else {
                collide::resolve(wd, &start, delta, m.on_ground, m.step_height())
            }
        };
        let ml = l2(mv);
        if ml > 1.0e-7 || l2(delta) - ml < 1.0e-7 {
            for (p, v) in m.pos.iter_mut().zip(mv) {
                *p += v;
            }
        }
        let xc = (delta[0] - mv[0]).abs() >= 1.0e-5;
        let zc = (delta[2] - mv[2]).abs() >= 1.0e-5;
        m.horizontal_collision = xc || zc;
        // The server is authoritative except for a mount the player pilots: ground and vertical
        // impact recomputed even without a vertical component (horizontal piston push).
        if delta[1].abs() > 0.0 || !Self::player_controls(m) {
            m.vertical_collision = delta[1] != mv[1];
            let below = m.vertical_collision && delta[1] < 0.0;
            m.on_ground = below;
            m.support = if below {
                let w = self.world();
                let lava = super::nether::strider::lava_stand(self, m, start.min[1]);
                let wd: &dyn rc_physics::World = match &lava {
                    Some(l) => l,
                    None => &w,
                };
                rc_physics::support_of(wd, &m.bbox(), m.pos)
            } else {
                None
            };
        }
        // Fall; F12: strider in lava, fall reset to zero.
        let effect_pos = self.mob_below(m, 0.2);
        let lava_reset = super::nether::strider::fall_reset(m);
        if lava_reset {
            m.fall_distance = 0.0;
        }
        if !m.in_water && mv[1] < 0.0 && !lava_reset {
            m.fall_distance -= f64::from(mv[1] as f32);
        }
        if m.on_ground && !lava_reset {
            if m.fall_distance > 0.0 {
                let on = self.st(effect_pos);
                self.fall_on(m, on, m.fall_distance);
                // Landing: state of the main supporting block, otherwise of the block at 0.2.
                let st = self.st(m.support.unwrap_or(effect_pos));
                self.game_event_src(
                    "minecraft:hit_ground",
                    m.pos,
                    Some(super::recent::vibration::Src::Living(Ent::Mob(m.id))),
                    Some(st),
                );
            }
            m.fall_distance = 0.0;
        }
        // Vertical collision: vertical velocity falls to 0 (no bounce for mobs
        // outside bouncing blocks).
        // Restitution: entity's own bounce (F03:
        // full sulfur cube) off walls, maximum with that of the block on the ground.
        if (delta[1].abs() > 0.0 && m.vertical_collision) || m.horizontal_collision {
            let es = self.st(effect_pos);
            let block_bounce = f64::from(self.phys.bounce.get(es as usize).copied().unwrap_or(0.0));
            let own = m.bounciness();
            let cur = m.vel;
            let mut v = cur;
            if xc {
                v[0] = -cur[0] * own;
            }
            if zc {
                v[2] = -cur[2] * own;
            }
            if m.vertical_collision {
                let mut r = own;
                if delta[1] < 0.0 {
                    r = if -cur[1] > m.attrs.gravity.value() && !self.btag_id(effect_pos, crate::tag_bits::TagId::SuppressesBounce) {
                        r.max(block_bounce)
                    } else {
                        0.0
                    };
                }
                v[1] = if r > 0.0 {
                    let portion = mv[1] / cur[1];
                    let comp = portion * m.attrs.gravity.value();
                    let air = if m.omnidirectional_air() { 0.91f32 } else { 0.98f32 };
                    let air = f64::from(corrected(air, m.air_drag_modifier()));
                    let drag = 1.0 + (air - 1.0) * portion;
                    (comp - cur[1]) * drag * r
                } else {
                    0.0
                };
            }
            m.vel = v;
            // Vertical bounce: `bounce` event.
            if m.vertical_collision && v[1] != 0.0 {
                self.mob_game_event(m, "minecraft:bounce");
            }
        }
        // Step.
        let h = (mv[0] * mv[0] + mv[2] * mv[2]).sqrt();
        let full = l2(mv).sqrt();
        let on = self.mob_below(m, 1.0e-5);
        let on_st = self.st(on);
        let climbing = self.climbable_state(on_st);
        m.move_dist += if climbing { (full * f64::from(0.6f32)) as f32 } else { (h * f64::from(0.6f32)) as f32 };
        m.surf.fly_dist += (full * f64::from(0.6f32)) as f32;
        if m.move_dist > m.next_step && !self.is_air(on) {
            // F09: swimming (swim sound) when the aquatic mob is not on the ground.
            if super::sealife::swim_step(self, m) {
                let f = self.speed_factor(m);
                m.vel[0] *= f64::from(f);
                m.vel[2] *= f64::from(f);
                return;
            }
            // Step on the ground: sound of the block at 0.2 under the feet, vibration of the supporting block (the game
            // emits it whatever the emission when both blocks coincide); otherwise swimming
            // in water; otherwise the step remains due.
            let sounds = super::aquatic::splash::emits_sounds(m.kind);
            let same = on == effect_pos;
            let es = self.st(effect_pos);
            let mut produced = self.mob_step_effects(m, effect_pos, es, sounds, same);
            if !same {
                produced |= self.mob_step_effects(m, on, on_st, false, true);
            }
            if produced {
                m.next_step = super::surface::next_step(m)
                    .or_else(|| super::nether::next_step(m))
                    .or_else(|| super::recent::next_step(m))
                    .unwrap_or_else(|| m.move_dist.trunc() + 1.0);
            } else if m.in_water {
                m.next_step = super::surface::next_step(m).unwrap_or_else(|| m.move_dist.trunc() + 1.0);
                if sounds {
                    self.mob_swim_sound(m);
                }
                self.mob_game_event(m, "minecraft:swim");
            }
        } else if self.is_air(on) {
            self.mob_flap(m);
        }
        let f = self.speed_factor(m);
        m.vel[0] *= f64::from(f);
        m.vel[2] *= f64::from(f);
    }

    /// Climbable block for step accumulation: `climbable` tag or powder snow.
    pub(crate) fn climbable_state(&self, st: u32) -> bool {
        self.phys.climbable(st) || self.data.block_of(st).is_some_and(|b| b.name == "minecraft:powder_snow")
    }

    /// Effects of a step on a block: nothing on air; on the
    /// ground or climbing, not swimming: sound and vibration according to the flags.
    fn mob_step_effects(&mut self, m: &mut Mob, p: [i32; 3], st: u32, sound: bool, vibrate: bool) -> bool {
        if self.v().is_air(st) {
            return false;
        }
        let climb = self.climbable_state(st);
        if !super::aquatic::splash::grounded_step(m, climb) {
            return false;
        }
        if sound {
            self.step_sound(m, p);
        }
        if vibrate {
            self.game_event_src(
                "minecraft:step",
                m.pos,
                Some(super::recent::vibration::Src::Living(Ent::Mob(m.id))),
                Some(st),
            );
        }
        true
    }

    /// The flap of wings above air: species' own flap, then
    /// `flap` event.
    fn mob_flap(&mut self, m: &mut Mob) {
        let flapping = match m.kind {
            Kind::Parrot | Kind::Chicken => m.surf.fly_dist > m.surf.next_flap,
            Kind::Bee => !m.on_ground && m.tick_count % 2 == 0,
            Kind::Allay => !m.on_ground,
            Kind::Bat => m.surf.bat_flags & 1 == 0 && m.tick_count % 10 == 0,
            Kind::Phantom => (m.id.wrapping_mul(3).wrapping_add(m.tick_count)) % 25 == 0,
            _ => false,
        };
        if !flapping {
            return;
        }
        match m.kind {
            Kind::Parrot => super::surface::parrot::on_flap(self, m),
            Kind::Chicken => m.surf.next_flap = m.surf.fly_dist + m.surf.flap_speed / 2.0,
            _ => {}
        }
        self.mob_game_event(m, "minecraft:flap");
    }

    /// "Fall on" block: rule of the touched block `on` (`fall_rule`), then damage.
    pub(crate) fn fall_on(&mut self, m: &mut Mob, on: u32, dist: f64) {
        // F14: breeze landing (sound beyond 3 blocks).
        if m.kind == Kind::Breeze {
            super::recent::breeze::on_fall(self, m, dist);
        }
        let honey = self.data.block_of(on).is_some_and(|b| b.name == "minecraft:honey_block");
        // Honey block: audible slide and jump particles (event 54), before the damage.
        if honey {
            self.mob_sound(m, "minecraft:block.honey_block.slide", 1.0, 1.0);
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Event { id: m.id, event: 54 }));
        }
        // Powder snow: no damage, species fall sound from 4 blocks (loud from 7).
        let Some((dist, mult, ty)) = self.fall_rule(on, dist, false) else {
            if dist >= 4.0 {
                let s = self.mob_fall_sound(m, dist >= 7.0);
                self.mob_sound(m, s, 1.0, 1.0);
            }
            return;
        };
        // F04: fall of equids (landing sound, riders hit too).
        if self.mount_fall(m, dist, mult) {
            return;
        }
        if m.kind.info().fall_immune {
            return;
        }
        let safe = m.attrs.safe_fall.value();
        // F10: goat −10, frog −5.
        let dmg = ((dist + 1.0e-6 - safe) * mult * m.attrs.fall_mult.value()).floor() as i32
            - super::brainy::fall_reduction(m.kind);
        if dmg > 0 {
            let s = self.mob_fall_sound(m, dmg > 4);
            self.mob_sound(m, s, 1.0, 1.0);
            self.mob_block_fall_sound(m);
            self.mob_hurt(m, ty, dmg as f32, None, None);
            // Honey: fall sound of the block replayed after the damage (volume ½, pitch ¾).
            if honey {
                self.mob_sound(m, "minecraft:block.honey_block.fall", 0.5, 0.75);
            }
        }
        // Creeper: the fall advances the fuse (1.5 tick per block, up to 5 ticks from the end).
        if m.kind == Kind::Creeper {
            m.swell = (m.swell + (dist * 1.5) as i32).min(m.fuse - 5);
        }
    }

    /// Fall sound of the species (small or big).
    fn mob_fall_sound(&self, m: &Mob, big: bool) -> &'static str {
        if m.kind.is_monster() && !super::sealife::handles(m.kind) {
            if big { "minecraft:entity.hostile.big_fall" } else { "minecraft:entity.hostile.small_fall" }
        } else if big {
            "minecraft:entity.generic.big_fall"
        } else {
            "minecraft:entity.generic.small_fall"
        }
    }

    /// Fall sound of the block touched at 0.2 under the feet: volume ½,
    /// pitch ¾ of its sound set; nothing on air.
    pub(crate) fn mob_block_fall_sound(&mut self, m: &Mob) {
        let below = super::mth::block_of([m.pos[0], m.pos[1] - 0.2, m.pos[2]]);
        if self.is_air(below) {
            return;
        }
        let st = self.st(below);
        let Some(t) = self.data.states.get(st as usize).and_then(|i| self.data.sound_types.get(usize::from(i.sound)))
        else {
            return;
        };
        if !t.fall.is_empty() {
            let (s, v, p) = (t.fall.clone(), t.volume * 0.5, t.pitch * 0.75);
            self.mob_sound(m, &s, v, p);
        }
    }

    fn step_sound(&mut self, m: &mut Mob, _on: [i32; 3]) {
        if self.mount_step_sound(m, _on) {
            return;
        }
        // F06: iron and copper golems (volume 1).
        // F14: breeze (events only) and silent happy ghasts.
        if super::recent::silent_steps(m.kind) {
            return;
        }
        if let Some((s, v)) = super::golems::step_sound(m)
            .or_else(|| super::nether::step_sound(m))
            .or_else(|| super::recent::step_sound(m))
        {
            self.mob_sound(m, s, v, 1.0);
            return;
        }
        // Sound variant set (pig, cow, chicken, wolf; babies included).
        let s = super::surface::step_sound(m)
            .map(str::to_owned)
            .or_else(|| self.variant_sound(m, "step_sound"))
            .or_else(|| m.kind.info().step_sound.map(str::to_owned));
        if let Some(s) = s {
            self.mob_sound(m, &s, 0.15, 1.0);
        }
    }

    /// Box of the entity in a powder snow block.
    pub(crate) fn mob_in_powder_snow(&self, m: &Mob) -> bool {
        let b = m.bbox();
        let lo = b.min.map(|v| (v + 1.0e-5).floor() as i32);
        let hi = b.max.map(|v| (v - 1.0e-5).floor() as i32);
        (lo[0]..=hi[0])
            .any(|x| (lo[1]..=hi[1]).any(|y| (lo[2]..=hi[2]).any(|z| self.bname([x, y, z]) == "minecraft:powder_snow")))
    }

    /// Freezing (server side): +1 per tick in powder snow up to 140, −2 otherwise
    /// or if the entity cannot freeze (`#freeze_immune_entity_types`, leather armor);
    /// slowdown proportional to freezing above a block; 1 freezing damage every
    /// 40 ticks once frozen.
    pub(crate) fn mob_freezing(&mut self, m: &mut Mob) {
        const REQUIRED: i32 = 140;
        let in_snow = self.mob_in_powder_snow(m);
        let can_freeze = !m.kind.freeze_immune()
            && !m.equipment[2..=6].iter().flatten().any(|s| self.item_tag(&s.id, "minecraft:freeze_immune_wearables"));
        if in_snow && can_freeze {
            m.ticks_frozen = (m.ticks_frozen + 1).min(REQUIRED);
        }
        if !in_snow || !can_freeze {
            m.ticks_frozen = (m.ticks_frozen - 2).max(0);
        }
        m.attrs.speed.remove_mod("minecraft:powder_snow");
        let below = self.bname(self.mob_below(m, 0.2));
        let on_block = !matches!(below, "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air");
        if m.ticks_frozen > 0 && on_block {
            let pct = m.ticks_frozen.min(REQUIRED) as f32 / REQUIRED as f32;
            m.attrs.speed.set_mod("minecraft:powder_snow", f64::from(-0.05 * pct), 0);
        }
        if m.tick_count % 40 == 0 && m.ticks_frozen >= REQUIRED && can_freeze {
            self.mob_hurt(m, "minecraft:freeze", 1.0, None, None);
        }
    }

    /// `travel(input)` for a mob with effective AI.
    pub(crate) fn mob_travel(&mut self, m: &mut Mob) {
        super::mounts::camel::freeze_travel(self, m); // F04
        // F12: ghast flight.
        if super::nether::travel(self, m) {
            return;
        }
        // F14: happy ghast flight (ridden or not).
        if super::recent::travel(self, m) {
            return;
        }
        if super::surface::travel_in_water(self, m) || super::aquatic::travel_in_water(self, m) {
            return;
        }
        // F09: squids (raw movement), swimmers in water.
        // F10: allay flight, swimming of the frog, axolotl and tadpole.
        if super::brainy::travel(self, m) {
            return;
        }
        if super::sealife::travel(self, m) {
            return;
        }
        let g = super::surface::gravity(m, m.attrs.gravity.value());
        let falling = m.vel[1] <= 0.0;
        let gravity = g;
        // F12: the strider whose cell holds lava walks on it like on the ground.
        if (m.in_water || m.lava_height > 0.0) && !super::nether::strider::stands_on_fluid(self, m) {
            let y0 = m.pos[1];
            if m.in_water {
                let slow = super::mounts::water_slow_down(m).unwrap_or_else(|| super::surface::water_slow_down(m));
                Self::move_relative(m, 0.02);
                let v = m.vel;
                self.mob_move(m, v);
                let mut v = m.vel;
                if m.horizontal_collision && self.on_climbable(m) {
                    v[1] = 0.2;
                }
                v = [v[0] * f64::from(slow), v[1] * f64::from(0.8f32), v[2] * f64::from(slow)];
                m.vel = Self::fluid_falling(gravity, falling, v);
            } else {
                Self::move_relative(m, 0.02);
                let v = m.vel;
                self.mob_move(m, v);
                if m.lava_height <= self.fluid_jump_threshold(m) {
                    m.vel = [m.vel[0] * 0.5, m.vel[1] * f64::from(0.8f32), m.vel[2] * 0.5];
                    m.vel = Self::fluid_falling(gravity, falling, m.vel);
                } else {
                    m.vel = m.vel.map(|v| v * 0.5);
                }
                if gravity != 0.0 {
                    m.vel[1] -= gravity / 4.0;
                }
            }
            // Leaving the fluid.
            if m.horizontal_collision {
                let b = m.bbox().offset(m.vel[0], m.vel[1] + f64::from(0.6f32) - m.pos[1] + y0, m.vel[2]);
                if rc_physics::is_free(&self.world(), &b) && !self.box_has_liquid(&b) {
                    m.vel[1] = f64::from(0.3f32);
                }
            }
            // `#can_float_while_ridden`: a mount carrying a passenger rises in water
            // deeper than its jump threshold (pilot or not).
            if !m.mount.passengers.is_empty()
                && m.kind.floats_while_ridden()
                && m.water_height > self.fluid_jump_threshold(m)
            {
                m.vel[1] += f64::from(0.04f32);
            }
            if m.kind == Kind::SulfurCube {
                self.sulfur_buoyancy(m);
            }
            return;
        }
        let below = self.mob_below(m, 0.500_001);
        let fb = if m.on_ground { corrected(self.friction_at(below), m.friction_modifier()) } else { 1.0 };
        let speed = if m.on_ground {
            if f64::from(fb) > 0.6 { m.speed * (0.216_000_02 / (fb * fb * fb)) } else { m.speed }
        } else {
            0.02
        };
        Self::move_relative(m, speed);
        // Ladders.
        let climb = self.on_climbable(m);
        if climb {
            m.fall_distance = 0.0;
            m.vel[0] = m.vel[0].clamp(-f64::from(0.15f32), f64::from(0.15f32));
            m.vel[2] = m.vel[2].clamp(-f64::from(0.15f32), f64::from(0.15f32));
            m.vel[1] = m.vel[1].max(-f64::from(0.15f32));
        }
        let v = m.vel;
        self.mob_move(m, v);
        let mut mv = m.vel;
        if (m.horizontal_collision || m.jumping) && self.on_climbable(m) {
            mv[1] = 0.2;
        }
        let mut my = mv[1];
        if self.state_at(below).is_some() {
            my -= gravity;
        } else {
            my = if m.pos[1] > f64::from(self.dim.height.min_y) { -0.1 } else { 0.0 };
        }
        let drag = m.air_drag_modifier();
        let air = corrected(0.91, drag);
        let fr = f64::from(fb * air);
        let vertical = super::surface::air_vertical_friction(m)
            .unwrap_or_else(|| if m.omnidirectional_air() { air } else { corrected(0.98, drag) });
        m.vel = [mv[0] * fr, my * f64::from(vertical), mv[2] * fr];
        // F10: long jump (friction ignored).
        if super::brainy::discard_friction(m) {
            m.vel = [mv[0], my, mv[2]];
        }
        // Chicken: slowed fall.
        if m.kind == Kind::Chicken && !m.on_ground && m.vel[1] < 0.0 {
            m.vel[1] *= 0.6;
        }
    }

    fn fluid_falling(g: f64, falling: bool, v: [f64; 3]) -> [f64; 3] {
        if g == 0.0 {
            return v;
        }
        let y = if falling && (v[1] - 0.005).abs() >= 0.003 && (v[1] - g / 16.0).abs() < 0.003 {
            -0.003
        } else {
            v[1] - g / 16.0
        };
        [v[0], y, v[2]]
    }

    pub(crate) fn fluid_jump_threshold(&self, m: &Mob) -> f64 {
        if m.kind == Kind::SulfurCube {
            return f64::from(m.height()) * 0.2;
        }
        if m.eye_height() < 0.4 { 0.0 } else { 0.4 }
    }

    /// A liquid in the box.
    pub(crate) fn box_has_liquid(&self, b: &rc_physics::Aabb) -> bool {
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.ceil() as i32 - 1);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if self.mob_fluid([x, y, z]) != Fluid::None {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Held jump (§ 1.1-6/7).
    pub(crate) fn mob_jump(&mut self, m: &mut Mob) {
        if !m.jumping {
            m.no_jump_delay = 0;
            return;
        }
        let in_lava = m.lava_height > 0.0;
        let fh = if in_lava { m.lava_height } else { m.water_height };
        let water_h = m.in_water && fh > 0.0;
        let thr = self.fluid_jump_threshold(m);
        if !water_h || (m.on_ground && fh <= thr) {
            if !in_lava || (m.on_ground && m.lava_height <= thr) {
                if (m.on_ground || (water_h && fh <= thr)) && m.no_jump_delay == 0 {
                    self.jump_from_ground(m);
                    m.no_jump_delay = 10;
                }
            } else {
                self.jump_in_liquid(m);
            }
        } else {
            self.jump_in_liquid(m);
        }
    }

    fn jump_in_liquid(&self, m: &mut Mob) {
        // F11: the bee only rises by 0.01.
        if super::wild::jump_in_liquid(m) {
            return;
        }
        // Magma in lava: imposed velocity according to size.
        if m.kind == Kind::MagmaCube && m.lava_height > 0.0 && !(m.in_water && m.water_height > 0.0) {
            m.vel[1] = f64::from(0.22f32 + m.cube.size as f32 * 0.05);
            m.needs_sync = true;
            return;
        }
        if m.nav.can_float {
            m.vel[1] += f64::from(0.04f32);
        } else {
            m.vel[1] += 0.3;
        }
    }

    /// Pushable.
    pub(crate) fn mob_pushable(&self, m: &Mob) -> bool {
        m.alive()
            && !self.on_climbable(m)
            && super::surface::pushable(m)
            && super::mounts::pushable(m)
            && super::recent::pushable(m)
    }

    /// Push at the end of the AI step (§ 1.4): crowding then mutual push.
    pub(crate) fn mob_push_entities(&mut self, m: &mut Mob) {
        if !super::surface::pushes(m) {
            return;
        }
        let bb = m.bbox();
        // (entity, slot of the list at the time of the record; u32::MAX for the player).
        let mut list: Vec<(Ent, u32)> = Vec::new();
        for (i, o) in self.mobs.list.iter().enumerate() {
            if let Some(o) = o
                && o.alive()
                && o.may_touch(&bb)
                && o.bbox().intersects(&bb)
                && self.mob_pushable(o)
                && !self.same_root(m, o)
            {
                list.push((Ent::Mob(o.id), i as u32));
            }
        }
        let player_box = self.player_aabb();
        if self.game_mode != 3
            && !self.life.vit.dead
            && player_box.intersects(&bb)
            && super::surface::pushes_player(m)
            && !self.player_same_root(m)
        {
            list.push((Ent::Player, u32::MAX));
        }
        if list.is_empty() {
            return;
        }
        let max = self.mob_rule_int("minecraft:max_entity_cramming", 24);
        if max > 0 && list.len() as i32 > max - 1 && m.rng.next_int_bound(4) == 0 && list.len() as i32 > max - 1 {
            self.mob_hurt(m, "minecraft:cramming", 6.0, None, None);
        }
        for (e, at) in list {
            // F14: the warden pushed against an entity gets irritated by it.
            if m.kind == Kind::Warden {
                super::recent::warden::on_push(self, m, e);
            }
            match e {
                Ent::Mob(id) => {
                    // Recorded slot still valid (same identifier), otherwise search: same
                    // slot as `mob_index` (identifiers are unique in the list).
                    let hit = push_idx()
                        && self.mobs.index.unique_ids()
                        && self.mobs.list.get(at as usize).and_then(Option::as_ref).is_some_and(|o| o.id == id);
                    let i = if hit { Some(at as usize) } else { self.mob_index(id) };
                    #[cfg(test)]
                    if super::index::verify() {
                        assert_eq!(i, super::index::scan_id(&self.mobs.list, id), "push: wrong slot of {id}");
                    }
                    let Some(i) = i else { continue };
                    let mut o = self.mobs.list[i].take().expect("mob");
                    // F06: the iron golem's push (sometimes targets the pushed enemy).
                    self.golem_pushed(m, &o);
                    let (po, pm) = (self.mob_pushable(&o), self.mob_pushable(m));
                    Self::push_pair(&mut o, m, po, pm);
                    // F06: a cube pushed by an iron golem is hurt by it (`push` of the cube).
                    self.cube_pushed_by_golem(&mut o, m);
                    self.mobs.list[i] = Some(o);
                }
                Ent::Player => {
                    // The player's push on the server side has no visible effect (the client
                    // pushes itself); the mob, on the other hand, is pushed.
                    let (xa, za) = Self::push_vec(self.pos, m.pos);
                    if let Some((xa, za)) = xa.zip(za)
                        && self.mob_pushable(m)
                    {
                        m.vel[0] += xa;
                        m.vel[2] += za;
                        m.needs_sync = true;
                    }
                }
            }
        }
    }

    fn push_vec(from: [f64; 3], to: [f64; 3]) -> (Option<f64>, Option<f64>) {
        let mut xa = to[0] - from[0];
        let mut za = to[2] - from[2];
        let mut dd = xa.abs().max(za.abs());
        if dd < f64::from(0.01f32) {
            return (None, None);
        }
        dd = dd.sqrt();
        xa /= dd;
        za /= dd;
        let pow = (1.0 / dd).min(1.0);
        xa *= pow * f64::from(0.05f32);
        za *= pow * f64::from(0.05f32);
        (Some(xa), Some(za))
    }

    /// `entity.push(this)`: `e` is the other, `src` the one who pushes.
    fn push_pair(e: &mut Mob, src: &mut Mob, e_pushable: bool, src_pushable: bool) {
        let (xa, za) = Self::push_vec(e.pos, src.pos);
        let (Some(xa), Some(za)) = (xa, za) else { return };
        if e_pushable {
            e.vel[0] -= xa;
            e.vel[2] -= za;
            e.needs_sync = true;
        }
        if src_pushable {
            src.vel[0] += xa;
            src.vel[2] += za;
            src.needs_sync = true;
        }
    }

    /// The player pushes the mobs they touch (in the AI step on the server side).
    pub(crate) fn player_pushes_mobs(&mut self) {
        if self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        let pb = self.player_aabb();
        let ppos = self.pos;
        let mut idx = Vec::new();
        for (i, o) in self.mobs.list.iter().enumerate() {
            if let Some(o) = o
                && o.alive()
                && o.may_touch(&pb)
                && o.bbox().intersects(&pb)
                && !self.player_same_root(o)
            {
                idx.push(i);
            }
        }
        for i in idx {
            let Some(mut o) = self.mobs.list[i].take() else { continue };
            let (xa, za) = Self::push_vec(o.pos, ppos);
            if let (Some(xa), Some(za)) = (xa, za)
                && self.mob_pushable(&o)
            {
                o.vel[0] -= xa;
                o.vel[2] -= za;
                o.needs_sync = true;
            }
            self.mobs.list[i] = Some(o);
        }
    }

    /// Width, height and eye height of the player's pose, single source: sleeping 0.2;
    /// swimming, gliding or spin attack 0.6 (eyes 0.4); sneaking 1.5 (1.27); standing 1.8
    /// (1.62). On the server side, death does not change the pose (the dead player keeps theirs).
    pub(crate) fn player_dims(&self) -> (f64, f64, f64) {
        if self.life.vit.sleeping.is_some() {
            let s = f64::from(0.2f32);
            (s, s, s)
        } else if self.mobs.sea.player_swimming || self.proj_player.fall_flying || self.proj_player.spin > 0 {
            (0.6, 0.6, f64::from(0.4f32))
        } else if self.sneaking {
            (0.6, 1.5, 1.27)
        } else {
            (0.6, 1.8, 1.62)
        }
    }

    pub(crate) fn player_aabb(&self) -> rc_physics::Aabb {
        let (w, h, _) = self.player_dims();
        let r = w / 2.0;
        rc_physics::Aabb {
            min: [self.pos[0] - r, self.pos[1], self.pos[2] - r],
            max: [self.pos[0] + r, self.pos[1] + h, self.pos[2] + r],
        }
    }

    pub(crate) fn player_eye_y(&self) -> f64 {
        self.pos[1] + self.player_eye_h()
    }

    /// Current player in water (box shrunk by 0.001 touching water) or in the rain
    /// (at the feet block, or at the top block of their box in the same column).
    pub(crate) fn player_in_water_or_rain(&self) -> bool {
        let b = self.player_aabb();
        let mut vel = [0.0; 3];
        let w = self.world();
        let (water, ..) =
            rc_physics::scan_fluids(&w, &self.phys, &b, self.pos, self.player_eye_y(), false, self.fast_lava(), &mut vel);
        if water > 0.0 {
            return true;
        }
        let feet = super::mth::block_of(self.pos);
        self.is_raining_at(feet) || self.is_raining_at([feet[0], b.max[1].floor() as i32, feet[2]])
    }

    pub(crate) fn player_eye_h(&self) -> f64 {
        self.player_dims().2
    }

    pub(crate) fn mob_rule_int(&self, name: &str, default: i32) -> i32 {
        let short = name.trim_start_matches("minecraft:");
        let v = self
            .rules
            .get(name)
            .or_else(|| self.rules.get(short))
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
        v.map_or(default, |v| v as i32)
    }
}
