//! Turtle (§ 4.4): amphibious navigation, swimming, home beach, mating without a baby (carried
//! egg), egg laying in the sand, growth (scute), deadly lightning.

use super::super::goals::Goal;
use super::super::goals::surface_goals::{Mtb, SGoal};
use super::super::mob::{Mob, MoveOp};
use super::super::mth;
use crate::server::Server;

pub fn register(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(0, Goal::Surf(SGoal::TurtlePanic { speed: 1.2, pos: [0.0; 3] }));
    g.add(1, super::breed_goal(1.0));
    g.add(1, Goal::Surf(SGoal::move_to_block(Mtb::TurtleLayEgg, 1.0, 16, 1, 0)));
    g.add(2, super::tempt(1.1, "minecraft:turtle_food", false));
    // Speed fixed at construction (never a baby at that moment): 1.0 for all.
    g.add(3, Goal::Surf(SGoal::move_to_block(Mtb::TurtleWater, 1.0, 24, 1, -1)));
    g.add(4, Goal::Surf(SGoal::TurtleGoHome { stuck: false, close: 0 }));
    g.add(7, Goal::Surf(SGoal::TurtleTravel { stuck: false }));
    g.add(8, super::look_player(8.0));
    g.add(9, Goal::Surf(SGoal::TurtleStroll { wanted: [0.0; 3] }));
}

/// The block below is sand (tag).
pub fn on_sand(s: &Server, p: [i32; 3]) -> bool {
    s.btag_id([p[0], p[1] - 1, p[2]], crate::tag_bits::TagId::Sand)
}

pub fn home_within(m: &Mob, d: f64) -> bool {
    let h = m.surf.home;
    let c = [f64::from(h[0]) + 0.5, f64::from(h[1]) + 0.5, f64::from(h[2]) + 0.5];
    mth::dist2(c, m.pos) < d * d
}

/// The speed setting (speed and advance).
fn set_speed(m: &mut Mob, v: f32) {
    m.speed = v;
    m.zza = v;
}

/// Turtle movement control (the operation stays "go to" once set).
pub fn move_control(_s: &Server, m: &mut Mob) {
    if m.in_water {
        m.vel[1] += 0.005;
        if !home_within(m, 16.0) {
            let v = (m.speed / 2.0).max(0.08);
            set_speed(m, v);
        }
        if m.is_baby() {
            let v = (m.speed / 3.0).max(0.06);
            set_speed(m, v);
        }
    } else if m.on_ground {
        let v = (m.speed / 2.0).max(0.06);
        set_speed(m, v);
    }
    if m.move_ctl.op == MoveOp::MoveTo && !m.nav.is_done() {
        let w = m.move_ctl.wanted;
        let (xd, mut yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = (xd * xd + yd * yd + zd * zd).sqrt();
        if dd < f64::from(1.0e-5f32) {
            set_speed(m, 0.0);
        } else {
            yd /= dd;
            let yr = (mth::atan2(zd, xd) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 - 90.0;
            m.yaw = mth::rotlerp(m.yaw, yr, 90.0);
            m.body_yaw = m.yaw;
            let target = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
            let v = m.speed + 0.125 * (target - m.speed);
            set_speed(m, v);
            m.vel[1] += f64::from(m.speed) * yd * 0.1;
        }
    } else {
        set_speed(m, 0.0);
    }
}

/// Swimming: push 0.1, damping 0.9, slow descent.
pub fn travel_in_water(s: &mut Server, m: &mut Mob) {
    let input = [f64::from(m.xxa), f64::from(m.yya), f64::from(m.zza)];
    let len = input.iter().map(|x| x * x).sum::<f64>();
    if len >= 1.0e-7 {
        let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
        let v = v.map(|x| x * f64::from(0.1f32));
        let (sn, c) = (mth::sin(m.yaw * mth::DEG), mth::cos(m.yaw * mth::DEG));
        m.vel[0] += v[0] * f64::from(c) - v[2] * f64::from(sn);
        m.vel[1] += v[1];
        m.vel[2] += v[2] * f64::from(c) + v[0] * f64::from(sn);
    }
    let v = m.vel;
    s.mob_move(m, v);
    m.vel = m.vel.map(|x| x * 0.9);
    if m.target.is_none() && (!m.surf.going_home || !home_within(m, 20.0)) {
        m.vel[1] -= 0.005;
    }
}

/// End of the AI step: sand particles while laying.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    if m.alive() && m.surf.laying && m.surf.lay_counter >= 1 && m.surf.lay_counter % 5 == 0 {
        let p = m.block_pos();
        if on_sand(s, p) {
            let below = s.st([p[0], p[1] - 1, p[2]]);
            s.level_event(2001, p, below as i32);
        }
    }
}

/// Mating: egg carried, no baby.
pub fn breed(s: &mut Server, m: &mut Mob, partner: i32) {
    s.s9_bred(m, partner, None); // S9: no baby, the egg will come later
    m.surf.has_egg = true;
    m.age = 6000;
    m.reset_love();
    if let Some(i) = s.mob_index(partner)
        && let Some(p) = s.mobs.list[i].as_mut()
    {
        p.age = 6000;
        p.reset_love();
    }
    if s.rule("minecraft:mob_drops", true) {
        let xp = m.rng.next_int_bound(7) + 1;
        s.spawn_orbs(m.pos, xp);
    }
}

/// End of laying: nest of 1 to 4 eggs above the sand.
pub fn lay_egg(s: &mut Server, m: &mut Mob, sand: [i32; 3]) {
    let tp = m.block_pos();
    let pitch = 0.9 + s.mobs.rng.next_float() * 0.2;
    let c = [f64::from(tp[0]) + 0.5, f64::from(tp[1]) + 0.5, f64::from(tp[2]) + 0.5];
    s.world_sound("minecraft:entity.turtle.lay_egg", 4, c, 0.3, pitch);
    let egg_pos = [sand[0], sand[1] + 1, sand[2]];
    let eggs = m.rng.next_int_bound(4) + 1;
    let st = s.default_state("minecraft:turtle_egg");
    let st = s.with_prop(st, "eggs", &eggs.to_string()).unwrap_or(st);
    s.set_block(egg_pos, st, 3);
    s.game_event_block("minecraft:block_place", egg_pos, Some(super::super::mob::Ent::Mob(m.id)), Some(st));
    m.surf.has_egg = false;
    m.surf.laying = false;
    m.surf.lay_counter = 0;
    m.in_love = 600;
}

/// Growing up: scute (`turtle_grow`) if the loot rule allows.
pub fn grew_up(s: &mut Server, m: &mut Mob) {
    if s.rule("minecraft:mob_drops", true) {
        let drops = super::loot(s, "minecraft:gameplay/turtle_grow", &crate::loot::Ctx::default());
        for d in drops {
            let r = &mut s.mobs.rng;
            let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
            s.spawn_item(m.pos, v, d, 10);
        }
    }
}
