//! Rabbit (§ 4.3): movement by jumps (own jump and movement controls), biome variants,
//! carrot raiding, killer bunny.

use rc_proto::nbt::Nbt;

use super::super::goals::surface_goals::SGoal;
use super::super::goals::{Goal, Melee, Who};
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{self, DEG};
use super::super::species::Kind;
use crate::server::Server;

pub const EVIL: i32 = 99;
const EVIL_MOD: &str = "minecraft:evil";

/// Known id, otherwise brown (the game's sparse table).
pub fn by_id(id: i32) -> i32 {
    if (0..=5).contains(&id) || id == EVIL { id } else { 0 }
}

pub fn register(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(1, Goal::Float);
    g.add(1, Goal::Surf(SGoal::ClimbPowderSnow));
    g.add(1, super::panic(2.2, "minecraft:panic_causes"));
    g.add(2, super::breed_goal(0.8));
    g.add(3, super::tempt(1.0, "minecraft:rabbit_food", false));
    g.add(4, super::avoid(Who::Player, 8.0, 2.2, 2.2));
    g.add(4, super::avoid(Who::Wolf, 10.0, 2.2, 2.2));
    g.add(4, super::avoid(Who::Monster, 4.0, 2.2, 2.2));
    g.add(5, Goal::Surf(SGoal::raid_garden()));
    g.add(6, super::stroll(0.6));
    g.add(11, super::look_player(10.0));
    if m.variant == EVIL {
        add_evil_goals(m);
    }
}

fn add_evil_goals(m: &mut Mob) {
    if m.goals.goals.iter().any(|w| matches!(w.goal, Goal::Melee { .. })) {
        return;
    }
    m.goals.add(4, super::melee(1.4, true, Melee::Plain));
    m.targets.add(1, Goal::HurtBy { base: super::super::goals::TBase::new(true, false), alert: true, stamp: 0 });
    m.targets.add(2, super::nearest(Who::Player, true, 10));
    m.targets.add(2, super::nearest(Who::Wolf, true, 10));
}

/// The killer gets armor, damage, attack goals and its name.
pub fn set_variant(m: &mut Mob, v: i32) {
    if v == EVIL {
        m.attrs.armor.base = 8.0;
        add_evil_goals(m);
        m.attrs.attack.set_mod(EVIL_MOD, 5.0, 0);
        if m.custom_name.is_none() {
            m.custom_name =
                Some(Nbt::Compound(vec![("translate".into(), Nbt::String("entity.minecraft.killer_bunny".into()))]));
        }
    } else {
        m.attrs.attack.remove_mod(EVIL_MOD);
    }
    m.variant = v;
}

/// Variant by biome (world random).
pub fn random_variant(s: &mut Server, p: [i32; 3]) -> i32 {
    let r = s.mobs.rng.next_int_bound(100);
    if s.biome_tag(p, "minecraft:spawns_white_rabbits") {
        if r < 80 { 1 } else { 3 }
    } else if s.biome_tag(p, "minecraft:spawns_gold_rabbits") {
        4
    } else if r < 50 {
        0
    } else if r < 90 {
        5
    } else {
        2
    }
}

/// The position setpoint of the rabbit's movement control (speed 1.5 in water).
pub fn set_wanted(m: &mut Mob, p: [f64; 3], speed: f64) {
    let speed = if m.in_water { 1.5 } else { speed };
    m.move_ctl.wanted = p;
    m.move_ctl.speed_mod = speed;
    if m.move_ctl.op != MoveOp::Jumping {
        m.move_ctl.op = MoveOp::MoveTo;
    }
    if speed > 0.0 {
        m.surf.next_jump_speed = speed;
    }
}

/// Navigation and movement control.
pub fn set_speed_modifier(m: &mut Mob, speed: f64) {
    m.nav.speed_mod = speed;
    let w = m.move_ctl.wanted;
    set_wanted(m, w, speed);
}

/// Start of the movement control tick.
pub fn move_control_pre(m: &mut Mob) {
    if m.on_ground && !m.jumping && !m.jump_ctl {
        set_speed_modifier(m, 0.0);
    } else if m.move_ctl.op == MoveOp::MoveTo || m.move_ctl.op == MoveOp::Jumping {
        let s = m.surf.next_jump_speed;
        set_speed_modifier(m, s);
    }
}

/// The "jumps" marking + jump duration; jump sound.
pub fn start_jumping(s: &mut Server, m: &mut Mob) {
    m.jumping = true;
    let p = ((m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0) * 0.8;
    let v = m.kind.info().volume;
    s.mob_sound(m, "minecraft:entity.rabbit.jump", v, p);
    m.surf.jump_duration = 15;
    m.surf.jump_ticks = 0;
}

fn face_point(m: &mut Mob, x: f64, z: f64) {
    m.yaw = (mth::atan2(z - m.pos[2], x - m.pos[0]) * 180.0 / std::f64::consts::PI) as f32 - 90.0;
}

/// The species' own server AI step.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    if m.surf.jump_delay > 0 {
        m.surf.jump_delay -= 1;
    }
    if m.surf.more_carrot > 0 {
        m.surf.more_carrot -= m.rng.next_int_bound(3);
        if m.surf.more_carrot < 0 {
            m.surf.more_carrot = 0;
        }
    }
    if m.on_ground {
        if !m.surf.was_on_ground {
            m.jumping = false;
            m.surf.jump_delay = if m.move_ctl.speed_mod < 2.2 { 10 } else { 3 };
            m.surf.can_jump = false;
        }
        if m.variant == EVIL
            && m.surf.jump_delay == 0
            && let Some(t) = m.target
            && let Some(tp) = s.ent_pos(t)
            && mth::dist2(m.pos, tp) < 16.0
        {
            face_point(m, tp[0], tp[2]);
            let sp = m.move_ctl.speed_mod;
            set_wanted(m, tp, sp);
            start_jumping(s, m);
            m.surf.was_on_ground = true;
        }
        if !m.jump_ctl {
            if m.move_ctl.op == MoveOp::MoveTo && m.surf.jump_delay == 0 {
                let mut pos = m.move_ctl.wanted;
                if let Some(p) = &m.nav.path
                    && !p.is_done()
                {
                    pos = p.entity_pos_at(m.width(), p.next);
                }
                face_point(m, pos[0], pos[2]);
                start_jumping(s, m);
            }
        } else if !m.surf.can_jump {
            m.surf.can_jump = true;
        }
    }
    m.surf.was_on_ground = m.on_ground;
    let _ = Ent::Player;
}

/// End of the AI step: jump progress.
pub fn post_ai_step(m: &mut Mob) {
    if m.surf.jump_ticks != m.surf.jump_duration {
        m.surf.jump_ticks += 1;
    } else if m.surf.jump_duration != 0 {
        m.surf.jump_ticks = 0;
        m.surf.jump_duration = 0;
        m.jumping = false;
    }
}

/// Base jump power (before the normalization by 0.42).
pub fn jump_power(_s: &Server, m: &Mob) -> f32 {
    let mut base = 0.3f32;
    if m.move_ctl.speed_mod <= 0.6 {
        base = 0.2;
    }
    if let Some(p) = &m.nav.path
        && !p.is_done()
        && p.entity_pos_at(m.width(), p.next)[1] > m.pos[1] + 0.5
    {
        base = 0.5;
    }
    if m.horizontal_collision || (m.jumping && m.move_ctl.wanted[1] > m.pos[1] + 0.5) {
        base = 0.5;
    }
    base
}

/// End of the jump from the ground: horizontal momentum, event 1 (leap).
pub fn after_jump(s: &mut Server, m: &mut Mob) {
    if m.move_ctl.speed_mod > 0.0 {
        let h = m.vel[0] * m.vel[0] + m.vel[2] * m.vel[2];
        if h < 0.01 {
            let input = [0.0, if m.is_baby() { 0.5 } else { 1.5 }, 1.0f64];
            let len = (input[0] * input[0] + input[1] * input[1] + input[2] * input[2]).sqrt();
            let v = input.map(|x| x / len * f64::from(0.1f32));
            let (sn, c) = (mth::sin(m.yaw * DEG), mth::cos(m.yaw * DEG));
            m.vel[0] += v[0] * f64::from(c) - v[2] * f64::from(sn);
            m.vel[1] += v[1];
            m.vel[2] += v[2] * f64::from(c) + v[0] * f64::from(sn);
        }
    }
    s.mob_event(m, 1);
}

/// Garden raiding: support block carrying a ripe carrot.
pub fn raid_target(s: &Server, p: [i32; 3]) -> bool {
    if !s.btag_id(p, crate::tag_bits::TagId::SupportsCrops) {
        return false;
    }
    let above = [p[0], p[1] + 1, p[2]];
    s.bname(above) == "minecraft:carrots" && s.data.state_properties(s.st(above)).any(|kv| kv == ("age", "7"))
}

/// Arrival at the carrot: eats one age (or destroys it).
pub fn eat_carrot(s: &mut Server, m: &mut Mob, p: [i32; 3]) {
    let crops = [p[0], p[1] + 1, p[2]];
    if s.bname(crops) == "minecraft:carrots" {
        let st = s.st(crops);
        let age: i32 =
            s.data.state_properties(st).find(|kv| kv.0 == "age").and_then(|kv| kv.1.parse().ok()).unwrap_or(0);
        if age == 0 {
            // Block turned to air then "destroyed": destroying air does nothing (no loot,
            // no particles), the game's default reproduced.
            let air = s.default_state("minecraft:air");
            s.set_block(crops, air, 2);
            s.destroy_block(crops, true, 512);
        } else {
            let to = s.with_prop(st, "age", &(age - 1).to_string()).unwrap_or(st);
            s.set_block(crops, to, 2);
            s.level_event(2001, crops, st as i32);
        }
        m.surf.more_carrot = 40;
    }
    let _ = Kind::Rabbit;
}
