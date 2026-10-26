//! Armadillo: brain (core, idle, rolled-up panic), states (idle, rolling up, scared,
//! unrolling) and their durations, fear detector (every 5 ticks), reduced damage when rolled up,
//! scute dropped every 6,000 to 11,999 ticks, brushing.

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::brain::*;
use crate::server::Server;

/// Armadillo states: name, threatened, animation duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Rolling,
    Scared,
    Unrolling,
}

impl State {
    pub fn of(v: u8) -> State {
        match v {
            1 => State::Rolling,
            2 => State::Scared,
            3 => State::Unrolling,
            _ => State::Idle,
        }
    }

    pub fn id(self) -> u8 {
        match self {
            State::Idle => 0,
            State::Rolling => 1,
            State::Scared => 2,
            State::Unrolling => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Rolling => "rolling",
            State::Scared => "scared",
            State::Unrolling => "unrolling",
        }
    }

    pub fn threatened(self) -> bool {
        self != State::Idle
    }

    pub fn anim(self) -> i64 {
        match self {
            State::Idle => 0,
            State::Rolling => 10,
            State::Scared => 50,
            State::Unrolling => 30,
        }
    }
}

pub fn state(m: &Mob) -> State {
    State::of(m.bm.armadillo)
}

pub fn is_scared(m: &Mob) -> bool {
    state(m) != State::Idle
}

/// State change (resets the counter to 0 if the value changes).
fn switch_to(m: &mut Mob, s: State) {
    if m.bm.armadillo != s.id() {
        m.bm.armadillo = s.id();
        m.bm.in_state = 0;
    }
}

pub fn brain(rng: &mut JRand) -> Brain {
    use K::*;
    use St::*;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::Swim { chance: 0.8 },
            Bk::Panic { speed: 2.0, armadillo: true, sniffer: false },
            Bk::LookSink { lo: 45, hi: 90 },
            Bk::MoveSink { lo: 150, hi: 250, cd: 0, path: None, last: None, speed: 0.0, armadillo: true },
            Bk::CountDown(TemptCd),
            Bk::CountDown(GazeCd),
            Bk::RollOut,
        ],
    );
    let idle = vec![
        at(0, Act::Idle, look_sometimes(6.0, true)),
        at(1, Act::Idle, Bk::MakeLove { speed: 1.0, close: 1, at: 0, sniffer: false }),
        at(
            2,
            Act::Idle,
            run_one(
                &[],
                0x4152_4d41,
                vec![
                    (Bk::Tempt { speed: Spd::Fixed(1.25), close: Close::Age(1.0, 2.0), sniffer: false }, 1),
                    (Bk::BabyFollow { lo: 5, hi: 16, speed: Spd::Fixed(1.25) }, 1),
                ],
            ),
        ),
        at(3, Act::Idle, Bk::RandomLook { lo: 150, hi: 250, yaw: 30.0, min_pitch: 0.0, range: 0.0 }),
        at(
            4,
            Act::Idle,
            run_one(
                &[(Walk, Absent)],
                0x4152_4d42,
                vec![
                    (Bk::Stroll { speed: 1.0, from_water: true }, 1),
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 1),
                    (Bk::DoNothing { lo: 30, hi: 60 }, 1),
                ],
            ),
        ),
    ];
    let panic = vec![at(0, Act::Panic, Bk::BallUp { peek: 0, was: false })];
    Brain::new(
        rng,
        &[Sn::Living, Sn::HurtBy, Sn::FoodTempt, Sn::Adult, Sn::ArmadilloScare],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Panic, &[(Danger, Present), (Panicking, Absent)], &[]), panic),
        ],
    )
}

/// Delay before the next scute (entity random draw): 6,000 to 11,999.
pub fn next_scute_time(m: &mut Mob) -> i32 {
    m.rng.next_int_bound(6000) + 6000
}

/// Neither panicking, nor in liquid, nor leashed, nor mounted, nor a passenger.
pub fn can_stay_rolled_up(_s: &Server, m: &Mob, b: &Brain) -> bool {
    !b.mem.panicking.has()
        && !m.in_water
        && m.lava_height <= 0.0
        && m.mount.vehicle.is_none()
        && m.mount.passengers.is_empty()
}

fn can_stay_rolled_up_mob(m: &Mob) -> bool {
    !super::is_panicking(m)
        && !m.in_water
        && m.lava_height <= 0.0
        && m.mount.vehicle.is_none()
        && m.mount.passengers.is_empty()
}

/// Within the enlarged 7/2/7 box; undead, last attacker, player (non-spectator) who sprints or
/// is mounted.
pub fn scared_by(s: &Server, m: &Mob, e: Ent) -> bool {
    let bb = m.bbox();
    let area = rc_physics::Aabb {
        min: [bb.min[0] - 7.0, bb.min[1] - 2.0, bb.min[2] - 7.0],
        max: [bb.max[0] + 7.0, bb.max[1] + 2.0, bb.max[2] + 7.0],
    };
    if !s.ent_bbox(e).is_some_and(|b| b.intersects(&area)) {
        return false;
    }
    match e {
        Ent::Mob(id) => {
            s.mob_ref(id).is_some_and(|o| o.kind.undead()) || m.last_hurt_by_mob.is_some_and(|(by, _)| by == e)
        }
        Ent::Player => {
            if m.last_hurt_by_mob.is_some_and(|(by, _)| by == e) {
                return true;
            }
            s.game_mode != 3 && (s.life.vit.sprinting || s.player_riding_any())
        }
    }
}

/// Stop in place, love forgotten, sound, rolling up.
pub fn roll_up(s: &mut Server, m: &mut Mob) {
    if is_scared(m) {
        return;
    }
    m.nav.stop();
    m.xxa = 0.0;
    m.yya = 0.0;
    m.speed = 0.0;
    m.zza = 0.0;
    m.vel = [0.0; 3];
    m.needs_sync = true;
    m.reset_love();
    s.make_sound(m, "minecraft:entity.armadillo.roll");
    switch_to(m, State::Rolling);
}

/// End-of-unroll sound, back to idle.
pub fn roll_out(s: &mut Server, m: &mut Mob) {
    if !is_scared(m) {
        return;
    }
    s.make_sound(m, "minecraft:entity.armadillo.unroll_finish");
    switch_to(m, State::Idle);
}

/// Ball tick (the "roll into a ball" behavior).
pub fn ball_up_tick(s: &mut Server, m: &mut Mob, b: &mut Brain, peek: &mut i32, was: &mut bool) {
    if *peek > 0 {
        *peek -= 1;
    }
    if state(m) == State::Rolling && m.bm.in_state > State::Rolling.anim() {
        switch_to(m, State::Scared);
        if m.on_ground {
            s.mob_sound(m, "minecraft:entity.armadillo.land", 1.0, 1.0);
        }
        return;
    }
    let st = state(m);
    // Memory absent: duration "never" (the danger always seems to be there).
    let ttl = b.mem.danger.ttl;
    let around = ttl > 75;
    if around != *was {
        *peek = State::Scared.anim() as i32 + m.rng.int_between(100, 400);
    }
    *was = around;
    if st == State::Scared {
        if *peek == 0 && m.on_ground && around {
            s.mob_event(m, 64);
            *peek = State::Scared.anim() as i32 + m.rng.int_between(100, 400);
        }
        if ttl < State::Unrolling.anim() {
            s.mob_sound(m, "minecraft:entity.armadillo.unroll_start", 1.0, 1.0);
            switch_to(m, State::Unrolling);
        }
    } else if st == State::Unrolling && ttl > State::Unrolling.anim() {
        switch_to(m, State::Scared);
    }
}

/// After the brain: scute (countdown only while alive; with no possible loot, the delay is not
/// re-armed).
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    if !m.alive() {
        return;
    }
    m.bm.scute_time -= 1;
    if m.bm.scute_time <= 0 && !m.is_baby() && s.rule("minecraft:mob_drops", true) {
        let drops = s.simple_loot("minecraft:gameplay/armadillo_shed", None);
        let any = !drops.is_empty();
        for d in drops {
            spawn_at_location(s, m, d);
        }
        if any {
            let p = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
            s.mob_sound(m, "minecraft:entity.armadillo.scute_drop", 1.0, p);
        }
        m.bm.scute_time = next_scute_time(m);
    }
}

/// Item at the position, random velocity (world random), delay 10.
pub fn spawn_at_location(s: &mut Server, m: &Mob, st: crate::player::Stack) {
    let r = &mut s.mobs.rng;
    let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
    s.spawn_item(m.pos, v, st, 10);
}

/// End of the common tick: head stuck to the body when rolled up, state counter.
pub fn post_tick(m: &mut Mob) {
    if is_scared(m) {
        m.head_yaw = m.body_yaw;
    }
    m.bm.in_state += 1;
}

/// After damage dealt: a living entity → danger 80 ticks and rolling up possible; an
/// environmental panic cause → unroll.
pub fn after_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) {
    if m.no_ai || m.health <= 0.0 {
        return;
    }
    if by.is_some() {
        if let Some(b) = m.bm.brain.as_mut() {
            b.mem.danger.set_exp(true, 80);
        }
        if can_stay_rolled_up_mob(m) {
            roll_up(s, m);
        }
    } else if s.dmg_tag(ty, "minecraft:panic_environmental_causes") {
        roll_out(s, m);
    }
}

/// Brush (adult: scute, brush −16), refusal when rolled up, otherwise animal.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item == "minecraft:brush" && !m.is_baby() {
        let drops = s.simple_loot("minecraft:gameplay/armadillo_brush", None);
        for d in drops {
            spawn_at_location(s, m, d);
        }
        s.mob_sound(m, "minecraft:entity.armadillo.brush", 1.0, 1.0);
        s.hurt_item(slot, 16);
        return Some(true);
    }
    if is_scared(m) {
        return Some(false);
    }
    s.bz_animal_interact(m, slot, item)
}
