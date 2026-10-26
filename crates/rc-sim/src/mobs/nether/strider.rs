//! Strider (`server-mobs.md` § 4.2): walks on lava (collision shape of half a
//! cell on the lava sources under it), cold out of lava (speed −34%), saddled mount
//! steered with a warped fungus on a stick (boost 140 to 980 ticks), breeding with
//! warped fungus, mounts at spawn (baby strider, zombified piglin rider).

use rc_physics::Aabb;

use super::super::env::Fluid;
use super::super::goals::{Goal, Who};
use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use super::NGoal;
use crate::player::Stack;
use crate::server::Server;

const SADDLE: usize = 7;
const STICK: &str = "minecraft:warped_fungus_on_a_stick";

/// Goals in the game's add order.
pub fn register_goals(m: &mut Mob) {
    use super::super::surface as sf;
    let g = &mut m.goals;
    g.add(1, sf::panic(1.65, "minecraft:panic_causes"));
    g.add(2, sf::breed_goal(1.0));
    g.add(3, sf::tempt(1.4, "minecraft:strider_tempt_items", false));
    g.add(4, NGoal::ToLava { next_start: 0, try_ticks: 0, pos: [0; 3], reached: false }.wrap());
    g.add(5, sf::follow_parent(1.0));
    g.add(7, Goal::Stroll { speed: 1.0, interval: 60, prob: None, wanted: [0.0; 3] });
    g.add(8, sf::look_player(8.0));
    g.add(8, sf::look_around());
    g.add(9, NGoal::LookAtStrider { at: None, time: 0 }.wrap());
    let _ = Who::Player;
}

/// Saddled (saddle slot occupied).
pub fn saddled(m: &Mob) -> bool {
    m.equipment[SADDLE].is_some()
}

fn tempted(m: &Mob) -> bool {
    m.goals.running(|g| matches!(g, Goal::Tempt { .. }))
}

fn panicking(m: &Mob) -> bool {
    m.goals.running(|g| matches!(g, Goal::Panic { .. }))
}

/// Ambient sound: silent when panicking or tempted.
pub fn ambient_sound(m: &Mob) -> Option<&'static str> {
    (!panicking(m) && !tempted(m)).then_some("minecraft:entity.strider.ambient")
}

/// Does the player hold the stick in a hand?
fn player_holds_stick(s: &Server) -> bool {
    [&s.slots[36 + s.held as usize], &s.slots[45]].iter().any(|h| h.as_ref().is_some_and(|h| h.id == STICK))
}

/// The passenger steering on the player's side: saddled, player first, stick in hand.
fn player_steers(s: &Server, m: &Mob) -> bool {
    saddled(m) && m.mount.passengers.first() == Some(&Ent::Player) && player_holds_stick(s)
}

/// Steering by the player (read by the mount engine).
pub fn player_controls(m: &Mob) -> bool {
    m.kind == Kind::Strider && m.neth.steered
}

/// Synced data and speed modifier (−34% of the base).
fn set_suffocating(m: &mut Mob, on: bool) {
    m.neth.suffocating = on;
    if on {
        m.attrs.speed.set_mod("minecraft:suffocating", -0.34f32 as f64, 1);
    } else {
        m.attrs.speed.remove_mod("minecraft:suffocating");
    }
}

/// A strider's tick before `super.tick`: sounds (entity's random source), cold, steering.
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    if tempted(m) && m.rng.next_int_bound(140) == 0 {
        s.make_sound(m, "minecraft:entity.strider.happy");
    } else if panicking(m) && m.rng.next_int_bound(60) == 0 {
        s.make_sound(m, "minecraft:entity.strider.retreat");
    }
    if !m.no_ai {
        let inside = s.btag_id(m.block_pos(), crate::tag_bits::TagId::StriderWarmBlocks);
        let on = s.btag_id(s.mob_below(m, 0.2), crate::tag_bits::TagId::StriderWarmBlocks);
        let warm = inside || on || m.lava_height > 0.0;
        let warm_vehicle =
            m.mount.vehicle.and_then(|v| s.mob_ref(v)).is_some_and(|v| v.kind == Kind::Strider && !v.neth.suffocating);
        // Speed modifier changed: sent back by the end-of-tick tracking.
        set_suffocating(m, !warm && !warm_vehicle);
    }
    m.neth.steered = player_steers(s, m);
}

/// Lava source (level 0) at `p`.
fn lava_source(s: &Server, p: [i32; 3]) -> bool {
    s.bname(p) == "minecraft:lava" && s.data.state_properties(s.st(p)).any(|(k, v)| k == "level" && v == "0")
}

/// World as seen by a strider: a lava source without lava above carries a shape of half
/// a cell when the strider's bottom is above it.
pub struct LavaStand<'a> {
    w: crate::play::SimWorld<'a>,
    s: &'a Server,
    bottom: f64,
}

impl rc_physics::World for LavaStand<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.w.state(x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        let over = self.w.collision(x, y, z, out);
        if out.is_empty()
            && self.bottom > f64::from(y) + 0.5 - f64::from(1.0e-5f32)
            && lava_source(self.s, [x, y, z])
            && self.s.mob_fluid([x, y + 1, z]) != Fluid::Lava
        {
            out.push(Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] });
        }
        over
    }

    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.w.moving_collision(x, y, z, out)
    }
}

/// Strider-specific collision world (entity's bottom at the start of the movement).
pub fn lava_stand<'a>(s: &'a Server, m: &Mob, bottom: f64) -> Option<LavaStand<'a>> {
    (m.kind == Kind::Strider).then(|| LavaStand { w: s.world(), s, bottom })
}

/// The false fluid-movement test: the strider whose cell carries lava walks (air).
pub fn stands_on_fluid(s: &Server, m: &Mob) -> bool {
    m.kind == Kind::Strider && s.mob_fluid(m.block_pos()) == Fluid::Lava
}

/// In lava, the fall is reset to zero.
pub fn fall_reset(m: &Mob) -> bool {
    m.kind == Kind::Strider && m.lava_height > 0.0
}

/// The strider's floating (end of tick): resting on the surface, otherwise rises.
pub fn float(s: &mut Server, m: &mut Mob) {
    if m.lava_height <= 0.0 {
        return;
    }
    let b = m.block_pos();
    let above_shape = m.pos[1] > f64::from(b[1]) + 0.5 - f64::from(1.0e-5f32);
    if above_shape && s.mob_fluid([b[0], b[1] + 1, b[2]]) != Fluid::Lava {
        m.on_ground = true;
    } else {
        m.vel = [m.vel[0] * 0.5, m.vel[1] * 0.5 + 0.05, m.vel[2] * 0.5];
    }
}

/// Lava 10; otherwise −∞ when already in lava, 0 elsewhere.
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> f32 {
    if s.mob_fluid(p) == Fluid::Lava {
        10.0
    } else if m.lava_height > 0.0 {
        f32::NEG_INFINITY
    } else {
        0.0
    }
}

/// Spawn rule: going up in lava, one arrives at air.
pub fn spawn_rule(s: &Server, p: [i32; 3]) -> bool {
    let mut y = p[1];
    loop {
        y += 1;
        if s.mob_fluid([p[0], y, p[2]]) != Fluid::Lava {
            break;
        }
    }
    s.is_air([p[0], y, p[2]])
}

/// Target of the "go to lava" goal: a lava block with a walkable cell above.
pub fn lava_target(s: &Server, p: [i32; 3]) -> bool {
    s.bname(p) == "minecraft:lava" && s.pathfindable_land([p[0], p[1] + 1, p[2]])
}

/// Eyes in lava, lava height included.
fn eyes_in_lava(s: &Server, m: &Mob) -> bool {
    let eye = m.eye_y();
    let p = super::super::mth::block_of([m.pos[0], eye, m.pos[2]]);
    if s.mob_fluid(p) != Fluid::Lava {
        return false;
    }
    let lvl = s
        .data
        .state_properties(s.st(p))
        .find(|(k, _)| *k == "level")
        .and_then(|(_, v)| v.parse::<i32>().ok())
        .unwrap_or(0);
    let above_lava = s.mob_fluid([p[0], p[1] + 1, p[2]]) == Fluid::Lava;
    let h = if above_lava {
        1.0
    } else if lvl >= 8 {
        8.0 / 9.0
    } else {
        f64::from(8 - lvl) / 9.0
    };
    eye < f64::from(p[1]) + h
}

/// Mount (saddled, without food), feed (common animal behavior), golden dandelion,
/// saddle; eating sound after an accepted food.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> bool {
    let food = s.item_tag(item, "minecraft:strider_food");
    if !food && saddled(m) && m.mount.passengers.is_empty() && !s.sneaking {
        if !eyes_in_lava(s, m) {
            s.player_start_riding(m);
        }
        return true;
    }
    let consumed = animal_interact(s, m, slot, item, food);
    if !consumed {
        return equip_saddle(s, m, slot);
    }
    if food {
        let p = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.2;
        let at = m.pos;
        s.world_sound("minecraft:entity.strider.eat", 6, at, 1.0, p);
    }
    true
}

/// The player's interaction with an animal then golden dandelion; true if the item was used.
fn animal_interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str, food: bool) -> bool {
    if food {
        if m.age == 0 && m.in_love <= 0 {
            if s.game_mode != 1 {
                s.consume_held(slot);
            }
            m.in_love = 600;
            m.love_cause = true;
            s.mob_event(m, 18);
            return true;
        }
        if m.is_baby() && !m.mount.age_locked {
            if s.game_mode != 1 {
                s.consume_held(slot);
            }
            let secs = super::super::mounts::feeding_secs(-m.age);
            super::super::mounts::age_up(m, secs, true);
            return true;
        }
    }
    // Golden dandelion: common case (delay of 40 ticks, particles).
    s.golden_dandelion(m, slot, item)
}

/// Saddle placed on right click: living adult, empty slot.
fn equip_saddle(s: &mut Server, m: &mut Mob, slot: usize) -> bool {
    let Some(h) = s.slots[slot].clone() else { return false };
    let Some(e) = s.equippable(&h) else { return false };
    if !e.on_interact || e.slot != SADDLE || m.equipment[SADDLE].is_some() || !m.alive() || m.is_baby() {
        return false;
    }
    if e.allowed.as_ref().is_some_and(|l| !s.entity_in(m.kind, l)) {
        return false;
    }
    let one = Stack { count: 1, ..h.clone() };
    // Placement by the item: in creative, the game works on a copy of the stack (nothing removed).
    s.consume_held(slot);
    m.drop_chance[SADDLE] = 2.0;
    s.mount_set_slot(m, SADDLE, Some(one));
    true
}

/// The horizontal escape vector (tabulated sines).
fn escape(collider: f64, colliding: f64, deg: f32) -> [f64; 3] {
    let d = (collider + colliding + f64::from(1.0e-5f32)) / 2.0;
    let a = deg * super::super::mth::DEG;
    let (s, c) = (-super::super::mth::sin(a), super::super::mth::cos(a));
    let m = s.abs().max(c.abs());
    [f64::from(s) * d / f64::from(m), 0.0, f64::from(c) * d / f64::from(m)]
}

/// Top of the shape, otherwise the one below − 1 (−∞ if low).
fn floor_height(s: &Server, p: [i32; 3]) -> f64 {
    if let Some(t) = s.collision_top(p) {
        return t;
    }
    match s.collision_top([p[0], p[1] - 1, p[2]]) {
        Some(b) if b >= 1.0 => b - 1.0,
        _ => f64::NEG_INFINITY,
    }
}

/// Five directions around the passenger's gaze (0, ±22.5,
/// ±45), cells from the top of the box down to 0.5 below the bottom, outside lava; otherwise on top.
pub fn dismount_location(s: &Server, v: &Mob, width: f32, poses: &[(f32, f32)], yaw: f32) -> [f64; 3] {
    let vw = f64::from(v.width());
    let dirs = [yaw, yaw - 22.5, yaw + 22.5, yaw - 45.0, yaw + 45.0].map(|d| escape(vw, f64::from(width), d));
    let b = v.bbox();
    let (top, bottom) = (b.max[1], b.min[1] - 0.5);
    let mut cells: Vec<[i32; 3]> = Vec::new();
    for d in dirs {
        let (x, z) = ((v.pos[0] + d[0]).floor() as i32, (v.pos[2] + d[2]).floor() as i32);
        let mut by = top.floor() as i32;
        let mut y = top;
        while y > bottom {
            let c = [x, by, z];
            if !cells.contains(&c) {
                cells.push(c);
            }
            by -= 1;
            y -= 1.0;
        }
    }
    for c in cells {
        if s.mob_fluid(c) == Fluid::Lava {
            continue;
        }
        let f = floor_height(s, c);
        if !(f.is_finite() && f < 1.0) {
            continue;
        }
        let loc = [f64::from(c[0]) + 0.5, f64::from(c[1]) + f, f64::from(c[2]) + 0.5];
        for &(w, h) in poses {
            // Free spot and within the world border.
            let b = Mob::bbox_at(loc, w, h);
            if rc_physics::is_free(&s.world(), &b) && s.box_in_border(&b) {
                return loc;
            }
        }
    }
    [v.pos[0], b.max[1], v.pos[2]]
}

/// Acceleration of a mount steered by an item (strider's random source); true if the boost starts.
pub fn boost(m: &mut Mob) -> bool {
    if m.neth.boosting {
        return false;
    }
    m.neth.boosting = true;
    m.neth.boost_time = 0;
    m.neth.boost_total = m.rng.next_int_bound(841) + 140;
    true
}

/// The acceleration tick (steered mount, every tick).
pub fn tick_boost(m: &mut Mob) {
    if m.neth.boosting {
        let t = m.neth.boost_time;
        m.neth.boost_time += 1;
        if t > m.neth.boost_total {
            m.neth.boosting = false;
        }
    }
}

/// Boost factor: 1 + 1.15 · sin(π · t / duration). The mounted speed is
/// computed by the steering client; the server only needs it for tests.
#[cfg(test)]
pub fn boost_factor(m: &Mob) -> f32 {
    if !m.neth.boosting {
        return 1.0;
    }
    let x = m.neth.boost_time as f32 / m.neth.boost_total as f32 * std::f32::consts::PI;
    1.0 + 1.15 * super::super::mth::sin(x)
}

/// Finalization (world random source): zombified piglin rider (1/30, if
/// monsters spawn), otherwise baby strider on the back (1/10); new group for each
/// adult (no baby drawn by the group).
pub fn finalize(s: &mut Server, m: &mut Mob, group: &mut super::super::spawn::Group) {
    if m.is_baby() {
        return;
    }
    let monsters = s.level.difficulty_id() != 0 && s.rule("minecraft:spawn_monsters", true);
    let jockey = s.mobs.rng.next_int_bound(30) == 0 && monsters;
    if jockey {
        // Zombie group data: baby drawn (5%) before the rider's finalization.
        let baby = s.mobs.rng.next_float() < 0.05;
        if let Some(k) = Kind::from_name("minecraft:zombified_piglin") {
            let mut r = s.new_mob(k, m.pos, m.yaw);
            r.pitch = 0.0;
            let mut g =
                super::super::spawn::Group { zombie_baby: Some(baby), zombie_no_jockey: true, ..Default::default() };
            s.finalize_spawn(&mut r, super::super::spawn::Reason::Jockey, &mut g);
            s.mob_start_riding(&mut r, m, true);
            r.equipment[0] = Some(Stack { id: STICK.into(), count: 1, components: None });
            s.add_mob(r);
        }
        m.equipment[SADDLE] = Some(Stack { id: "minecraft:saddle".into(), count: 1, components: None });
        m.drop_chance[SADDLE] = 2.0;
        group.ageable = true;
        group.size = 0;
        group.no_baby = true;
    } else if s.mobs.rng.next_int_bound(10) == 0 {
        let mut b = s.new_mob(Kind::Strider, m.pos, m.yaw);
        b.age = -24000;
        b.pitch = 0.0;
        let mut g = super::super::spawn::Group::default();
        s.finalize_spawn(&mut b, super::super::spawn::Reason::Jockey, &mut g);
        s.mob_start_riding(&mut b, m, true);
        s.add_mob(b);
        group.ageable = true;
        group.size = 0;
        group.no_baby = true;
    } else {
        // New animal spawn group (baby probability 0.5): size 0, hence never a baby nor a draw.
        group.ageable = true;
        group.size = 0;
        group.no_baby = false;
        group.baby_chance = Some(0.5);
    }
}

impl Server {
    /// Boost of the steered strider; the stick wears (1) and becomes a
    /// fishing rod again when it breaks. True if the item is the stick (action handled).
    pub(crate) fn strider_boost_use(&mut self, hand: i32) -> bool {
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        if self.slots[slot].as_ref().is_none_or(|s| s.id != STICK) {
            return false;
        }
        let Some(vid) = self.mobs.mounts.riding.vehicle else { return true };
        let Some(i) = self.mob_index(vid) else { return true };
        let Some(mut v) = self.mobs.list[i].take() else { return true };
        let ok = v.kind == Kind::Strider && player_steers(self, &v) && boost(&mut v);
        self.mobs.list[i] = Some(v);
        if ok {
            let before = self.slots[slot].clone();
            self.hurt_item(slot, 1);
            if self.slots[slot].is_none()
                && let Some(mut b) = before
            {
                // Broken: fishing rod (components copied, wear reset to zero).
                crate::items::remove_component(&mut b, "minecraft:damage");
                b.id = "minecraft:fishing_rod".into();
                b.count = 1;
                self.slots[slot] = Some(b);
            }
            self.broadcast_menus();
        }
        true
    }
}
