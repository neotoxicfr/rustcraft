//! Camel and camel husk: brain (sensors, memories, "core" and "idle" activities, behaviors in the
//! game's priority order), sitting posture (40- and 52-tick transitions), dash (`dash`) and its
//! 55-tick cooldown.

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{DEG, cos, dist2, sin, wrap};
use super::super::species::Kind;
use crate::server::Server;

/// Tracking of a target: entity (eyes or feet) or block center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Track {
    Ent(Ent, bool),
    Pos([i32; 3]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WalkTarget {
    pub target: Track,
    pub speed: f32,
    pub close: i32,
}

/// State of a behavior with a duration.
#[derive(Debug, Clone, Copy, Default)]
struct B {
    running: bool,
    end: i64,
}

/// Camel brain.
#[derive(Debug, Clone)]
pub struct Brain {
    // Sensors: delay before the next sweep (entities, hurt, tempting, adult).
    sensors: [i64; 4],
    started: bool,
    // Memories.
    pub look: Option<Track>,
    pub walk: Option<WalkTarget>,
    path: bool,
    cant_reach: Option<i64>,
    tempting: bool,
    tempt_cd: Option<i32>,
    gaze_cd: Option<i32>,
    pub panicking: bool,
    hurt_by: Option<String>,
    hurt_by_entity: Option<Ent>,
    pub breed: Option<i32>,
    nearest: Option<Vec<Ent>>,
    /// Entities seen (visibility cache valid until the next sweep).
    visible: Option<Vec<(Ent, Option<bool>)>>,
    adult: Option<i32>,
    // Behaviors: core 0..5, idle 0..4.
    swim: B,
    panic: B,
    look_sink: B,
    move_sink: B,
    move_cooldown: i32,
    move_has_path: bool,
    move_last: Option<[i32; 3]>,
    move_speed: f32,
    cd_tempt: B,
    cd_gaze: B,
    look_ticker: i32,
    love: B,
    love_at: i64,
    /// First single choice (tempting, baby following): shuffled order, children running.
    one_a: OneOf<2>,
    look_around: B,
    /// Second single choice (wandering, walking toward the gaze, sitting down, doing nothing).
    one_b: OneOf<4>,
}

/// List shuffled by its own random, only one child started.
#[derive(Debug, Clone)]
struct OneOf<const N: usize> {
    running: bool,
    order: [usize; N],
    rng: JRand,
    child: [B; N],
}

impl<const N: usize> OneOf<N> {
    fn new(salt: i64) -> Self {
        OneOf {
            running: false,
            order: std::array::from_fn(|i| i),
            rng: JRand::from_time(salt),
            child: [B::default(); N],
        }
    }

    /// The shuffle of a weighted list (weight 1): stable sort by −U descending.
    fn shuffle(&mut self) {
        let mut keyed: Vec<(f64, usize)> = self.order.iter().map(|&i| (-f64::from(self.rng.next_float()), i)).collect();
        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        for (k, (_, i)) in keyed.into_iter().enumerate() {
            self.order[k] = i;
        }
    }
}

impl Default for Brain {
    fn default() -> Self {
        Brain {
            sensors: [0; 4],
            started: false,
            look: None,
            walk: None,
            path: false,
            cant_reach: None,
            tempting: false,
            tempt_cd: None,
            gaze_cd: None,
            panicking: false,
            hurt_by: None,
            hurt_by_entity: None,
            breed: None,
            nearest: None,
            visible: None,
            adult: None,
            swim: B::default(),
            panic: B::default(),
            look_sink: B::default(),
            move_sink: B::default(),
            move_cooldown: 0,
            move_has_path: false,
            move_last: None,
            move_speed: 0.0,
            cd_tempt: B::default(),
            cd_gaze: B::default(),
            look_ticker: 0,
            love: B::default(),
            love_at: 0,
            one_a: OneOf::new(0x4341_4D41),
            look_around: B::default(),
            one_b: OneOf::new(0x4341_4D42),
        }
    }
}

/// Initial sensor delays (entity random at construction).
pub fn init_brain(m: &mut Mob) {
    let mut b = Brain::default();
    for s in &mut b.sensors {
        *s = i64::from(m.rng.next_int_bound(20));
    }
    b.started = true;
    m.mount.brain = Some(Box::new(b));
}

// ---------- Posture ----------

pub fn sitting(m: &Mob) -> bool {
    m.mount.last_pose_tick < 0
}

pub fn pose_time(s: &Server, m: &Mob) -> i64 {
    s.time - m.mount.last_pose_tick.abs()
}

pub fn in_transition(s: &Server, m: &Mob) -> bool {
    pose_time(s, m) < if sitting(m) { 40 } else { 52 }
}

/// Sitting or in transition.
pub fn refuse_to_move(s: &Server, m: &Mob) -> bool {
    sitting(m) || in_transition(s, m)
}

fn camel_sound(m: &Mob, sit: bool) -> &'static str {
    match (m.kind == Kind::Camel, sit) {
        (true, true) => "minecraft:entity.camel.sit",
        (true, false) => "minecraft:entity.camel.stand",
        (false, true) => "minecraft:entity.camel_husk.sit",
        (false, false) => "minecraft:entity.camel_husk.stand",
    }
}

pub fn sit_down(s: &mut Server, m: &mut Mob) {
    if !sitting(m) {
        s.make_sound(m, camel_sound(m, true));
        m.mount.camel_sitting_pose = true;
        m.mount.last_pose_tick = -s.time;
    }
}

pub fn stand_up(s: &mut Server, m: &mut Mob) {
    if sitting(m) {
        s.make_sound(m, camel_sound(m, false));
        m.mount.camel_sitting_pose = false;
        m.mount.last_pose_tick = s.time;
    }
}

pub fn stand_up_instantly(s: &Server, m: &mut Mob) {
    m.mount.camel_sitting_pose = false;
    m.mount.last_pose_tick = (s.time - 52 - 1).max(0);
}

/// The box of the target posture is free.
pub fn can_change_pose(s: &Server, m: &Mob) -> bool {
    let (w, h) = if sitting(m) {
        let d = if m.is_baby() { super::baby_dims(m.kind) } else { (1.7, 2.375, 2.275) };
        (d.0, d.1)
    } else if m.is_baby() {
        (0.95, 0.425)
    } else {
        (1.7, 2.375 - 1.43)
    };
    let b = Mob::bbox_at(m.pos, w, h);
    let b = rc_physics::Aabb { min: b.min.map(|v| v + 1.0e-7), max: b.max.map(|v| v - 1.0e-7) };
    rc_physics::is_free(&s.world(), &b)
}

/// End of a camel's tick: end of the dash, cooldown, clamped head, standing up in water.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    let st = &mut m.mount;
    if st.dash && st.dash_cooldown < 50 && (m.on_ground || m.in_water || m.lava_height > 0.0 || st.vehicle.is_some()) {
        set_dash(m, false);
    }
    if m.mount.dash_cooldown > 0 {
        m.mount.dash_cooldown -= 1;
        if m.mount.dash_cooldown == 0 {
            let snd = if m.kind == Kind::Camel {
                "minecraft:entity.camel.dash_ready"
            } else {
                "minecraft:entity.camel_husk.dash_ready"
            };
            let p = m.block_pos().map(|v| f64::from(v) + 0.5);
            s.world_sound(snd, 6, p, 1.0, 1.0);
        }
    }
    if refuse_to_move(s, m) {
        m.head_yaw = super::super::mth::rotate_if_necessary(m.head_yaw, m.body_yaw, 30.0);
    }
    if sitting(m) && m.in_water {
        stand_up_instantly(s, m);
    }
}

/// Dash: any change re-arms the cooldown at 55 if it is empty.
pub fn set_dash(m: &mut Mob, v: bool) {
    if m.mount.dash != v {
        m.mount.dash = v;
        if m.mount.dash_cooldown == 0 {
            m.mount.dash_cooldown = 55;
        }
    }
}

/// `travel`: motionless on the ground as long as it refuses to move.
pub fn freeze_travel(s: &Server, m: &mut Mob) {
    if super::is_camel(m.kind) && refuse_to_move(s, m) && m.on_ground {
        m.vel[0] = 0.0;
        m.vel[2] = 0.0;
        m.xxa = 0.0;
        m.zza = 0.0;
    }
}

/// Movement control: stands up to go somewhere (not leashed, free space).
pub fn move_control_pre(s: &mut Server, m: &mut Mob) {
    if m.move_ctl.op == MoveOp::MoveTo && sitting(m) && !in_transition(s, m) && can_change_pose(s, m) {
        stand_up(s, m);
    }
}

// ---------- Brain ----------

fn track_pos(s: &Server, t: Track) -> Option<[f64; 3]> {
    match t {
        Track::Ent(e, eye) => {
            let p = s.ent_pos(e)?;
            Some(if eye { [p[0], s.ent_eye(e)?, p[2]] } else { p })
        }
        Track::Pos(b) => Some([f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5]),
    }
}

fn track_block(s: &Server, t: Track) -> Option<[i32; 3]> {
    match t {
        Track::Ent(e, _) => s.ent_pos(e).map(super::super::mth::block_of),
        Track::Pos(b) => Some(b),
    }
}

fn manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
}

impl Server {
    /// The species' own server AI step for the camel: brain tick (core + idle activities).
    pub(crate) fn camel_brain_tick(&mut self, m: &mut Mob) {
        let Some(mut b) = m.mount.brain.take() else { return };
        if !b.started {
            b.started = true;
        }
        self.camel_sensors(m, &mut b);
        let t = self.time;
        self.camel_start_behaviors(m, &mut b, t);
        self.camel_tick_behaviors(m, &mut b, t);
        m.mount.brain = Some(b);
    }

    /// Duration drawn at start (world random).
    fn behavior_duration(&mut self, min: i32, max: i32) -> i64 {
        i64::from(min + self.mobs.rng.next_int_bound(max + 1 - min))
    }

    fn visible_contains(&self, m: &mut Mob, b: &mut Brain, e: Ent) -> bool {
        let Some(v) = b.visible.as_mut() else { return false };
        let Some(slot) = v.iter_mut().find(|(x, _)| *x == e) else { return false };
        if let Some(r) = slot.1 {
            return r;
        }
        // Shared range of the sensors (rewritten by the last sensor that ticked).
        let range = self.mobs.pig.sensor_range;
        let r = self.targeting_ok(m, e, false, range, true, true);
        if let Some(v) = b.visible.as_mut()
            && let Some(slot) = v.iter_mut().find(|(x, _)| *x == e)
        {
            slot.1 = Some(r);
        }
        r
    }

    /// Closest seen entity satisfying `f`.
    fn visible_closest(&self, m: &mut Mob, b: &mut Brain, f: &dyn Fn(&Server, Ent) -> bool) -> Option<Ent> {
        let list: Vec<Ent> = b.visible.as_ref()?.iter().map(|x| x.0).collect();
        list.into_iter().find(|&e| f(self, e) && self.visible_contains(m, b, e))
    }

    fn camel_sensors(&mut self, m: &mut Mob, b: &mut Brain) {
        // Each sensor that ticks rewrites the shared range of the targeting conditions.
        if b.sensors.iter().any(|&t| t <= 1) {
            self.mobs.pig.sensor_range = m.follow_range();
        }
        // Nearby living entities (tracking range), sorted by distance.
        b.sensors[0] -= 1;
        if b.sensors[0] <= 0 {
            b.sensors[0] = 20;
            let r = m.follow_range();
            let bb = m.bbox();
            let area = rc_physics::Aabb { min: bb.min.map(|v| v - r), max: bb.max.map(|v| v + r) };
            let mut list: Vec<(f64, Ent)> = self
                .mobs
                .list
                .iter()
                .flatten()
                .filter(|o| o.id != m.id && o.alive() && o.touches(&area))
                .map(|o| (dist2(m.pos, o.pos), Ent::Mob(o.id)))
                .collect();
            if self.ent_alive(Ent::Player) && self.player_aabb().intersects(&area) {
                list.push((dist2(m.pos, self.pos), Ent::Player));
            }
            list.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
            let ents: Vec<Ent> = list.into_iter().map(|x| x.1).collect();
            b.visible = Some(ents.iter().map(|e| (*e, None)).collect());
            b.nearest = Some(ents);
        }
        // Recent hurt (40 ticks).
        b.sensors[1] -= 1;
        if b.sensors[1] <= 0 {
            b.sensors[1] = 20;
            match &m.last_hit {
                Some(h) if self.time - h.tick as i64 <= 40 => {
                    b.hurt_by = Some(h.ty.clone());
                    if let Some(e) = h.by {
                        b.hurt_by_entity = Some(e);
                    }
                }
                _ => b.hurt_by = None,
            }
            if b.hurt_by_entity.is_some_and(|e| !self.ent_alive(e)) {
                b.hurt_by_entity = None;
            }
        }
        // Tempting player (food in hand, tempt range, not a passenger).
        b.sensors[2] -= 1;
        if b.sensors[2] <= 0 {
            b.sensors[2] = 20;
            let range = m.attrs.tempt_range.value();
            let food = super::food_tag(m.kind);
            let held = [&self.slots[36 + self.held as usize], &self.slots[45]];
            let holds = held.iter().any(|s| s.as_ref().is_some_and(|s| self.item_tag(&s.id, food)));
            let ok = self.game_mode != 3
                && holds
                && !m.mount.passengers.contains(&Ent::Player)
                && self.targeting_ok(m, Ent::Player, false, range, false, true);
            b.tempting = ok;
        }
        // Closest adult seen, of the same species.
        b.sensors[3] -= 1;
        if b.sensors[3] <= 0 {
            b.sensors[3] = 20;
            if b.visible.is_some() {
                let kind = m.kind;
                let f = |s: &Server, e: Ent| match e {
                    Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| o.kind == kind && !o.is_baby()),
                    Ent::Player => false,
                };
                b.adult =
                    self.visible_closest(m, b, &f).and_then(|e| if let Ent::Mob(id) = e { Some(id) } else { None });
            }
        }
    }

    /// Start of the stopped behaviors, by priority (core then idle at equal priority).
    fn camel_start_behaviors(&mut self, m: &mut Mob, b: &mut Brain, t: i64) {
        // 0: swim; look at a player from time to time.
        if !b.swim.running && self.camel_swim_ok(m) {
            b.swim.running = true;
            b.swim.end = t + self.behavior_duration(60, 60);
        }
        if b.look.is_none() && b.visible.is_some() {
            let me = m.pos;
            let f = move |s: &Server, e: Ent| e == Ent::Player && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= 36.0);
            if let Some(target) = self.visible_closest(m, b, &f) {
                let fire = if b.look_ticker == 0 {
                    b.look_ticker = self.mobs.rng.next_int_bound(31) + 30 - 1;
                    false
                } else {
                    b.look_ticker -= 1;
                    b.look_ticker == 0
                };
                if fire {
                    b.look = Some(Track::Ent(target, true));
                }
            }
        }
        // 1: panic; mate.
        if !b.panic.running && self.camel_panic_ok(m, b) {
            b.panic.running = true;
            b.panic.end = t + self.behavior_duration(100, 120);
            stand_up_instantly(self, m);
            b.panicking = true;
            b.walk = None;
            m.nav.stop();
        }
        if !b.love.running && b.visible.is_some() && b.breed.is_none() && !b.panicking && m.in_love > 0 {
            let me = m.clone();
            let f = move |s: &Server, e: Ent| match e {
                Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| {
                    o.kind == Kind::Camel
                        && camel_can_mate(&me, o)
                        && !o.mount.brain.as_ref().is_some_and(|x| x.panicking)
                }),
                Ent::Player => false,
            };
            if let Some(Ent::Mob(pid)) = self.visible_closest(m, b, &f) {
                b.love.running = true;
                b.love.end = t + self.behavior_duration(110, 110);
                b.breed = Some(pid);
                self.lock_gaze_to_partner(m, b, pid);
                b.love_at = t + i64::from(60 + m.rng.next_int_bound(50));
            }
        }
        // 2: look at the target; tempting or baby following (only one).
        if !b.look_sink.running && b.look.is_some() {
            b.look_sink.running = true;
            b.look_sink.end = t + self.behavior_duration(45, 90);
        }
        if !b.one_a.running {
            b.one_a.shuffle();
            for k in 0..2 {
                let i = b.one_a.order[k];
                if b.one_a.child[i].running {
                    continue;
                }
                let ok = match i {
                    0 => b.tempt_cd.is_none() && b.tempting && b.breed.is_none() && !b.panicking,
                    _ => !refuse_to_move(self, m) && self.camel_baby_follow(m, b),
                };
                if ok {
                    b.one_a.child[i].running = true;
                    b.one_a.child[i].end = if i == 0 { t + self.behavior_duration(60, 60) } else { t };
                    b.one_a.running = true;
                    break;
                }
            }
        }
        // 3: go to the walk target; look around.
        if !b.move_sink.running && b.walk.is_some() && !b.path && self.move_sink_start_ok(m, b) {
            b.move_sink.running = true;
            b.move_sink.end = t + self.behavior_duration(150, 250);
            b.path = true;
            let s = f64::from(b.move_speed);
            let p = m.nav.path.clone();
            self.move_sink_move(m, p, s);
        }
        if !b.look_around.running && b.look.is_none() && b.gaze_cd.is_none() {
            b.look_around.running = true;
            b.look_around.end = t + self.behavior_duration(60, 60);
            let pitch = (m.rng.next_float() * 0.0 + 0.0).clamp(-90.0, 90.0);
            let yaw = wrap(m.yaw + 2.0 * m.rng.next_float() * 30.0 - 30.0);
            let dir = direction(pitch, yaw);
            let eye = [m.pos[0], m.eye_y(), m.pos[2]];
            let p = [eye[0] + dir[0], eye[1] + dir[1], eye[2] + dir[2]];
            b.look = Some(Track::Pos(super::super::mth::block_of(p)));
            b.gaze_cd = Some(m.rng.next_int_bound(101) + 150);
        }
        // 4: tempt countdown; wander, walk toward the gaze, sit down, nothing.
        if !b.cd_tempt.running && b.tempt_cd.is_some() {
            b.cd_tempt.running = true;
            b.cd_tempt.end = t + self.behavior_duration(60, 60);
        }
        if !b.one_b.running && b.walk.is_none() {
            b.one_b.shuffle();
            for k in 0..4 {
                let i = b.one_b.order[k];
                if b.one_b.child[i].running {
                    continue;
                }
                let ok = match i {
                    0 => {
                        !refuse_to_move(self, m) && {
                            let p = self.land_random_pos(m, 10, 7);
                            b.walk = p.map(|p| WalkTarget {
                                target: Track::Pos(super::super::mth::block_of(p)),
                                speed: 2.0,
                                close: 0,
                            });
                            true
                        }
                    }
                    1 => {
                        !refuse_to_move(self, m)
                            && match b.look {
                                Some(l) => {
                                    b.walk = Some(WalkTarget { target: l, speed: 2.0, close: 3 });
                                    true
                                }
                                None => false,
                            }
                    }
                    2 => {
                        let ok = !m.in_water
                            && pose_time(self, m) >= 400
                            && m.on_ground
                            && !Self::player_controls(m)
                            && Self::mob_controller(m).is_none()
                            && can_change_pose(self, m);
                        if ok {
                            b.one_b.child[2].end = t + self.behavior_duration(60, 60);
                            if sitting(m) {
                                stand_up(self, m);
                            } else if !b.panicking {
                                sit_down(self, m);
                            }
                        }
                        ok
                    }
                    _ => {
                        b.one_b.child[3].end = t + self.behavior_duration(30, 60);
                        true
                    }
                };
                if ok {
                    b.one_b.child[i].running = true;
                    if i < 2 {
                        b.one_b.child[i].end = t;
                    }
                    b.one_b.running = true;
                    break;
                }
            }
        }
        // 5: gaze countdown.
        if !b.cd_gaze.running && b.gaze_cd.is_some() {
            b.cd_gaze.running = true;
            b.cd_gaze.end = t + self.behavior_duration(60, 60);
        }
    }

    /// Water targeted by the panic of a burning animal: nothing if its feet are in a solid
    /// block; otherwise the first water block in a box of 5 (horizontal) by 1 (vertical),
    /// traversed by increasing Manhattan distance (x increasing, then y, then positive z
    /// before its mirror). An animal whose width rounds to 2 also requires water to the south,
    /// east and south-east.
    fn camel_panic_water(&self, m: &Mob) -> Option<[i32; 3]> {
        use super::super::env::Fluid;
        let o = m.block_pos();
        if self.has_collision(o) {
            return None;
        }
        let water = |p: [i32; 3]| self.mob_fluid(p) == Fluid::Water;
        let wide = m.width().ceil() as i32 == 2;
        let (rx, ry, rz): (i32, i32, i32) = (5, 1, 5);
        for d in 0..=rx + ry + rz {
            let mx = rx.min(d);
            for x in -mx..=mx {
                let my = ry.min(d - x.abs());
                for y in -my..=my {
                    let z = d - x.abs() - y.abs();
                    if z > rz {
                        continue;
                    }
                    let zs: &[i32] = if z == 0 { &[0] } else { &[z, -z] };
                    for &zz in zs {
                        let p = [o[0] + x, o[1] + y, o[2] + zz];
                        if !water(p) {
                            continue;
                        }
                        if !wide
                            || (water([p[0], p[1], p[2] + 1])
                                && water([p[0] + 1, p[1], p[2]])
                                && water([p[0] + 1, p[1], p[2] + 1]))
                        {
                            return Some(p);
                        }
                    }
                }
            }
        }
        None
    }

    /// Tick or stop of the running behaviors, in the same order.
    fn camel_tick_behaviors(&mut self, m: &mut Mob, b: &mut Brain, t: i64) {
        if b.swim.running {
            if t > b.swim.end || !self.camel_swim_ok(m) {
                b.swim.running = false;
            } else if m.rng.next_float() < 0.8 {
                m.jump_ctl = true;
            }
        }
        if b.panic.running {
            if t > b.panic.end {
                b.panic.running = false;
                b.panicking = false;
            } else if m.nav.is_done() {
                // On fire: the nearest water first, otherwise a random land point.
                let water = if m.on_fire() { self.camel_panic_water(m) } else { None };
                let p = water.or_else(|| self.land_random_pos(m, 5, 4).map(super::super::mth::block_of));
                if let Some(p) = p {
                    b.walk = Some(WalkTarget { target: Track::Pos(p), speed: 4.0, close: 0 });
                }
            }
        }
        if b.love.running {
            let ok = b.breed.is_some_and(|pid| {
                self.mob_ref(pid).is_some_and(|p| {
                    p.alive()
                        && p.kind == Kind::Camel
                        && camel_can_mate(m, p)
                        && !p.mount.brain.as_ref().is_some_and(|x| x.panicking)
                })
            }) && b.breed.is_some_and(|pid| self.visible_contains(m, b, Ent::Mob(pid)))
                && t <= b.love_at
                && !b.panicking;
            if t > b.love.end || !ok {
                b.love.running = false;
                b.breed = None;
                b.walk = None;
                b.look = None;
                b.love_at = 0;
            } else if let Some(pid) = b.breed {
                self.lock_gaze_to_partner(m, b, pid);
                if let Some(pp) = self.mob_ref(pid).map(|p| p.pos)
                    && dist2(m.pos, pp) < 9.0
                    && t >= b.love_at
                {
                    self.mount_breed(m, pid);
                    b.breed = None;
                    if let Some(i) = self.mob_index(pid)
                        && let Some(o) = self.mobs.list[i].as_mut()
                        && let Some(ob) = o.mount.brain.as_mut()
                    {
                        ob.breed = None;
                    }
                }
            }
        }
        if b.look_sink.running {
            let visible = match b.look {
                Some(Track::Ent(e, _)) => self.ent_alive(e) && self.visible_contains(m, b, e),
                Some(Track::Pos(_)) => true,
                None => false,
            };
            if t > b.look_sink.end || !visible {
                b.look_sink.running = false;
                b.look = None;
            } else if let Some(p) = b.look.and_then(|l| track_pos(self, l)) {
                self.look_at(m, p, 10.0, 40.0);
            }
        }
        if b.one_a.running {
            let mut any = false;
            for i in 0..2 {
                if !b.one_a.child[i].running {
                    continue;
                }
                if i == 0 {
                    let ok = b.tempting && b.breed.is_none() && !b.panicking;
                    if !ok {
                        b.one_a.child[0].running = false;
                        b.tempt_cd = Some(100);
                        b.walk = None;
                        b.look = None;
                        continue;
                    }
                    b.look = Some(Track::Ent(Ent::Player, true));
                    let close = if m.is_baby() { 2.5 } else { 3.5 };
                    if dist2(m.pos, self.pos) < close * close {
                        b.walk = None;
                    } else {
                        b.walk = Some(WalkTarget { target: Track::Ent(Ent::Player, false), speed: 2.5, close: 2 });
                    }
                    any = true;
                } else {
                    b.one_a.child[1].running = false;
                }
            }
            if !any {
                b.one_a.running = false;
            }
        }
        if b.move_sink.running {
            let reached = b.walk.and_then(|w| track_block(self, w.target).map(|tb| (tb, w.close)));
            let ok = b.move_has_path
                && b.move_last.is_some()
                && !m.nav.is_done()
                && matches!(reached, Some((tb, c)) if manhattan(tb, m.block_pos()) > c);
            if t > b.move_sink.end || !ok {
                b.move_sink.running = false;
                if let Some((tb, c)) = reached
                    && manhattan(tb, m.block_pos()) > c
                    && m.nav.stuck
                {
                    b.move_cooldown = self.mobs.rng.next_int_bound(40);
                }
                m.nav.stop();
                b.walk = None;
                b.path = false;
                b.move_has_path = false;
            } else if let Some(w) = b.walk
                && let (Some(tb), Some(last)) = (track_block(self, w.target), b.move_last)
            {
                let d = [tb[0] - last[0], tb[1] - last[1], tb[2] - last[2]];
                if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 4 && self.try_compute_path(m, b, w) {
                    b.move_last = Some(tb);
                    let s = f64::from(b.move_speed);
                    let p = m.nav.path.clone();
                    self.move_sink_move(m, p, s);
                }
            }
        }
        if b.look_around.running && t > b.look_around.end {
            b.look_around.running = false;
        }
        if b.cd_tempt.running {
            match b.tempt_cd {
                Some(n) if n > 0 => b.tempt_cd = Some(n - 1),
                _ => {
                    b.cd_tempt.running = false;
                    b.tempt_cd = None;
                }
            }
        }
        if b.one_b.running {
            let mut any = false;
            for i in 0..4 {
                if !b.one_b.child[i].running {
                    continue;
                }
                let done = match i {
                    0..=2 => true,
                    _ => t > b.one_b.child[3].end,
                };
                if done {
                    b.one_b.child[i].running = false;
                } else {
                    any = true;
                }
            }
            if !any {
                b.one_b.running = false;
            }
        }
        if b.cd_gaze.running {
            match b.gaze_cd {
                Some(n) if n > 0 => b.gaze_cd = Some(n - 1),
                _ => {
                    b.cd_gaze.running = false;
                    b.gaze_cd = None;
                }
            }
        }
    }

    fn camel_swim_ok(&self, m: &Mob) -> bool {
        (m.in_water && m.water_height > self.fluid_jump_threshold(m)) || m.lava_height > 0.0
    }

    fn camel_panic_ok(&self, m: &Mob, b: &Brain) -> bool {
        let hurt = b.hurt_by.as_ref().is_some_and(|ty| self.dmg_tag(ty, "minecraft:panic_causes"));
        (hurt || b.panicking) && !super::mob_controlled(m)
    }

    /// Baby following an adult seen between 5 and 17 blocks away (walk 2.5, stop at 4).
    fn camel_baby_follow(&self, m: &Mob, b: &mut Brain) -> bool {
        if !m.is_baby() || b.walk.is_some() {
            return false;
        }
        let Some(a) = b.adult else { return false };
        let Some(p) = self.mob_ref(a).map(|o| o.pos) else { return false };
        let d = dist2(m.pos, p);
        if (5.0 * 5.0..17.0 * 17.0).contains(&d) {
            b.look = Some(Track::Ent(Ent::Mob(a), true));
            b.walk = Some(WalkTarget { target: Track::Ent(Ent::Mob(a), false), speed: 2.5, close: 4 });
            return true;
        }
        false
    }

    fn lock_gaze_to_partner(&mut self, m: &mut Mob, b: &mut Brain, pid: i32) {
        b.look = Some(Track::Ent(Ent::Mob(pid), true));
        b.walk = Some(WalkTarget { target: Track::Ent(Ent::Mob(pid), true), speed: 1.0, close: 2 });
        let me = m.id;
        if let Some(i) = self.mob_index(pid)
            && let Some(o) = self.mobs.list[i].as_mut()
            && let Some(ob) = o.mount.brain.as_mut()
        {
            ob.breed = Some(me);
            ob.look = Some(Track::Ent(Ent::Mob(me), true));
            ob.walk = Some(WalkTarget { target: Track::Ent(Ent::Mob(me), true), speed: 1.0, close: 2 });
        }
    }

    /// The start conditions of the movement toward the target.
    fn move_sink_start_ok(&mut self, m: &mut Mob, b: &mut Brain) -> bool {
        if b.move_cooldown > 0 {
            b.move_cooldown -= 1;
            return false;
        }
        let Some(w) = b.walk else { return false };
        let Some(tb) = track_block(self, w.target) else { return false };
        let reached = manhattan(tb, m.block_pos()) <= w.close;
        if !reached && self.try_compute_path(m, b, w) {
            b.move_last = Some(tb);
            return true;
        }
        b.walk = None;
        if reached {
            b.cant_reach = None;
        }
        false
    }

    fn try_compute_path(&mut self, m: &mut Mob, b: &mut Brain, w: WalkTarget) -> bool {
        let Some(tb) = track_block(self, w.target) else { return false };
        let path = self.create_path(m, tb, 0);
        b.move_speed = w.speed;
        if manhattan(tb, m.block_pos()) <= w.close || path.as_ref().is_some_and(|p| p.reached) {
            b.cant_reach = None;
        } else if b.cant_reach.is_none() {
            b.cant_reach = Some(self.time);
        }
        b.move_has_path = path.is_some();
        m.nav.path_to = None;
        // Path kept for `start` (movement handed over to the navigation).
        if let Some(p) = path {
            m.nav.path = Some(p);
            return true;
        }
        false
    }

    fn move_sink_move(&mut self, m: &mut Mob, path: Option<super::super::nav::Path>, speed: f64) {
        self.move_to_path(m, path, speed);
    }
}

/// The camel's possible-mating test: another camel, both ready.
fn camel_can_mate(m: &Mob, o: &Mob) -> bool {
    o.id != m.id && super::feed::can_parent(m) && super::feed::can_parent(o)
}

/// The direction vector for a pitch and a yaw.
fn direction(pitch: f32, yaw: f32) -> [f64; 3] {
    let f = cos(-yaw * DEG - std::f32::consts::PI);
    let g = sin(-yaw * DEG - std::f32::consts::PI);
    let h = -cos(-pitch * DEG);
    let i = sin(-pitch * DEG);
    [f64::from(g * h), f64::from(i), f64::from(f * h)]
}
