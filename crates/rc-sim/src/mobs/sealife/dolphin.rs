//! Dolphin: smooth swimming (swimmer movement and look controls), 4,800-tick air
//! and surface breathing, moisture out of water, jumps, swimming with the player (dolphin's
//! grace), fish given → treasure (ruins and shipwrecks), playing with items, retaliation.

use super::super::goals::{Melee, Who};
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{DEG, cos, dist2, facing, pitch_to, rotate_towards, rotlerp_360, sin, wrap, yaw_to};
use super::goals::AGoal;
use crate::player::Stack;
use crate::server::Server;

pub const MAX_AIR: i32 = 4800;
pub const MAX_MOIST: i32 = 2400;

/// Goals in the game's insertion order (the goals tied to boats and guardians, non-simulated
/// entities, never have any effect and are omitted).
pub fn register(m: &mut Mob) {
    use super::super::surface::{look_around, look_player, melee};
    m.goals.add(0, AGoal::BreathAir.wrap());
    m.goals.add(0, AGoal::FindLiquid.wrap());
    m.goals.add(1, AGoal::Treasure { stuck: false }.wrap());
    m.goals.add(2, AGoal::SwimWithPlayer { on: false }.wrap());
    m.goals.add(4, AGoal::swim(1.0, 10));
    m.goals.add(4, look_around());
    m.goals.add(5, look_player(6.0));
    m.goals.add(5, AGoal::Jump { interval: super::super::mth::reduced(10), breached: false }.wrap());
    m.goals.add(6, melee(1.2, true, Melee::Plain));
    m.goals.add(7, AGoal::MoveToItem { cooldown: 0 }.wrap());
    m.goals.add(8, AGoal::PlayWithItems.wrap());
    m.goals.add(8, AGoal::FollowRidden { following: false, recalc: 0, toward: false }.wrap());
    m.goals.add(
        9,
        super::super::goals::Goal::Avoid {
            who: Who::Exact(super::super::species::Kind::Guardian),
            max: 8.0,
            walk: 1.0,
            sprint: 1.0,
            from: None,
            path: None,
        },
    );
    m.targets.add(1, super::super::goals::Goal::HurtBy {
        base: super::super::goals::TBase::new(true, false),
        alert: true,
        stamp: 0,
    });
    let _ = Who::Player;
}

// ---------- Controls ----------

/// The smooth swim control (turns of 85° in pitch, 10° in yaw, compensated gravity).
pub fn smooth_move_control(m: &mut Mob, in_water: f32, outside: f32) {
    if m.in_water {
        m.vel[1] += 0.005;
    }
    if m.move_ctl.op == MoveOp::MoveTo && !m.nav.is_done() {
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < f64::from(2.500_000_3e-7f32) {
            m.zza = 0.0;
            return;
        }
        let yr = yaw_to(xd, zd);
        m.yaw = rotlerp_360(m.yaw, yr, 10.0);
        m.body_yaw = m.yaw;
        m.head_yaw = m.yaw;
        let speed = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
        if m.in_water {
            m.speed = speed * in_water;
            let sd = (xd * xd + zd * zd).sqrt();
            if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
                let xr = pitch_to(yd, sd);
                let xr = wrap(xr).clamp(-85.0, 85.0);
                m.pitch = rotate_towards(m.pitch, xr, 5.0);
            }
            let (c, s) = (cos(m.pitch * DEG), sin(m.pitch * DEG));
            m.zza = c * speed;
            m.yya = -s * speed;
        } else {
            let left = wrap(m.yaw - yr).abs();
            let factor = 1.0 - ((left - 10.0) / 50.0).clamp(0.0, 1.0);
            m.speed = speed * outside * factor;
            m.zza = m.speed;
        }
    } else {
        m.speed = 0.0;
        m.xxa = 0.0;
        m.yya = 0.0;
        m.zza = 0.0;
    }
}

/// The smooth swim look control (head/body difference of 10° at most, the body follows by 4°).
pub fn smooth_look_control(m: &mut Mob) {
    if m.look.cooldown > 0 {
        m.look.cooldown -= 1;
        let w = m.look.wanted;
        let (xd, zd) = (w[0] - m.pos[0], w[2] - m.pos[2]);
        if zd.abs() > f64::from(1.0e-5f32) || xd.abs() > f64::from(1.0e-5f32) {
            m.head_yaw = rotate_towards(m.head_yaw, yaw_to(xd, zd) + 20.0, m.look.y_max);
        }
        let yd = w[1] - m.eye_y();
        let sd = (xd * xd + zd * zd).sqrt();
        if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
            m.pitch = rotate_towards(m.pitch, pitch_to(yd, sd) + 10.0, m.look.x_max);
        }
    } else {
        if m.nav.is_done() {
            m.pitch = rotate_towards(m.pitch, 0.0, 5.0);
        }
        m.head_yaw = rotate_towards(m.head_yaw, m.body_yaw, m.look.y_max);
    }
    let d = wrap(m.head_yaw - m.body_yaw);
    if d < -10.0 {
        m.body_yaw -= 4.0;
    } else if d > 10.0 {
        m.body_yaw += 4.0;
    }
}

// ---------- Tick ----------

/// End of a dolphin's tick: moisture (rain or water), drying out, leaps on the ground.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if m.no_ai {
        m.air = MAX_AIR;
        return;
    }
    if s.mob_in_water_or_rain(m) {
        m.sea.moist = MAX_MOIST;
    } else {
        m.sea.moist -= 1;
        if m.sea.moist <= 0 {
            s.mob_hurt(m, "minecraft:dry_out", 1.0, None, None);
        }
        if m.on_ground {
            let dx = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.2);
            let dz = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.2);
            m.vel[0] += dx;
            m.vel[1] += 0.5;
            m.vel[2] += dz;
            m.yaw = m.rng.next_float() * 360.0;
            m.on_ground = false;
            m.needs_sync = true;
        }
    }
}

/// Pickup: empty hand, item without delay in range, `mob_griefing` rule.
pub fn pickup(s: &mut Server, m: &mut Mob) {
    if !m.can_pickup || !m.alive() || m.dead || !s.rule("minecraft:mob_griefing", true) || m.equipment[0].is_some() {
        return;
    }
    let bb = m.bbox();
    let hit = s
        .life
        .drops
        .iter()
        .find(|d| {
            !d.is_removed()
                && d.stack.count > 0
                && d.pickup_delay <= 0
                && d.pos[0] + 0.125 > bb.min[0] - 1.0
                && d.pos[0] - 0.125 < bb.max[0] + 1.0
                && d.pos[1] + 0.25 > bb.min[1]
                && d.pos[1] < bb.max[1]
                && d.pos[2] + 0.125 > bb.min[2] - 1.0
                && d.pos[2] - 0.125 < bb.max[2] + 1.0
        })
        .map(|d| (d.id, d.stack.clone()));
    let Some((id, st)) = hit else { return };
    m.equipment[0] = Some(st.clone());
    m.drop_chance[0] = 2.0;
    s.mob_take(id, m.id, st.count);
}

/// Fish given: the baby grows up, the adult looks for treasure.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item.is_empty() || !s.item_tag(item, "minecraft:fishes") {
        return None;
    }
    s.mob_sound(m, "minecraft:entity.dolphin.eat", 1.0, 1.0);
    if m.is_baby() && !m.mount.age_locked {
        s.consume_held(slot);
        let secs = super::super::mounts::feeding_secs(-m.age);
        super::super::mounts::age_up(m, secs, true);
    } else {
        m.sea.got_fish = true;
        s.consume_held(slot);
    }
    Some(true)
}

// ---------- Goals ----------

/// "Playable" items: without pickup delay, in the water, within 8 blocks of the box.
fn items_near(s: &Server, m: &Mob) -> Vec<(i32, [f64; 3])> {
    let b = m.bbox();
    let area = rc_physics::Aabb { min: b.min.map(|v| v - 8.0), max: b.max.map(|v| v + 8.0) };
    s.life
        .drops
        .iter()
        .filter(|d| {
            let ib = rc_physics::Aabb {
                min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
            };
            !d.is_removed()
                && d.pickup_delay <= 0
                && ib.intersects(&area)
                && s.mob_fluid(super::super::mth::block_of(d.pos)) == super::super::env::Fluid::Water
        })
        .map(|d| (d.id, d.pos))
        .collect()
}

impl Server {
    /// Is the player swimming (state kept up to date by `sealife_player_tick`)?
    pub(crate) fn player_swimming(&self) -> bool {
        self.mobs.sea.player_swimming
    }

    /// Held item dropped in front of the dolphin (40-tick delay, gaze momentum).
    fn dolphin_drop_item(&mut self, m: &mut Mob) -> bool {
        let Some(st) = m.equipment[0].take() else { return false };
        self.dolphin_throw(m, st);
        true
    }

    fn dolphin_throw(&mut self, m: &mut Mob, st: Stack) {
        let y = m.eye_y() - f64::from(0.3f32);
        // Starting velocity of the item (item random), immediately replaced.
        let _ = (self.life.rng.next_double(), self.life.rng.next_double());
        let dir = m.rng.next_float() * (std::f32::consts::PI * 2.0);
        let pow2 = 0.02 * m.rng.next_float();
        let (yr, xr) = (m.yaw * DEG, m.pitch * DEG);
        let v = [
            f64::from(0.3 * -sin(yr) * cos(xr) + cos(dir) * pow2),
            f64::from(0.3 * sin(xr) * 1.5),
            f64::from(0.3 * cos(yr) * cos(xr) + sin(dir) * pow2),
        ];
        self.spawn_item([m.pos[0], y, m.pos[2]], v, st, 40);
    }

    pub(crate) fn dolphin_can_use(&mut self, g: &mut AGoal, m: &mut Mob) -> bool {
        match g {
            AGoal::BreathAir => m.air < 140,
            // Tag `dolphin_tries_to_find`: source water only (flowing water is not).
            AGoal::FindLiquid => m.on_ground && !self.mob_source_water(m.block_pos()),
            AGoal::Treasure { .. } => m.sea.got_fish && m.air >= 100,
            AGoal::SwimWithPlayer { on } => {
                *on = self.ent_alive(Ent::Player)
                    && self.game_mode != 3
                    && dist2(m.pos, self.pos) <= 100.0
                    && self.player_swimming()
                    && m.target != Some(Ent::Player);
                *on
            }
            AGoal::Jump { interval, .. } => {
                if m.rng.next_int_bound(*interval) != 0 {
                    return false;
                }
                // Corridor in the direction the dolphin looks, not in that of its velocity: a
                // mostly vertical velocity only controlled its own column, and it jumped out
                // of a narrow channel to land on the bank.
                let (sx, sz) = facing(m.yaw);
                let p = m.block_pos();
                for i in [0, 1, 4, 5, 6, 7] {
                    let q = [p[0] + sx * i, p[1], p[2] + sz * i];
                    let water = self.mob_fluid(q) == super::super::env::Fluid::Water
                        && !self.btag_id(q, crate::tag_bits::TagId::BlocksDolphinJump);
                    let clear = self.is_air([q[0], q[1] + 1, q[2]]) && self.is_air([q[0], q[1] + 2, q[2]]);
                    if !water || !clear {
                        return false;
                    }
                }
                true
            }
            AGoal::MoveToItem { cooldown } => *cooldown <= m.tick_count && !items_near(self, m).is_empty(),
            AGoal::PlayWithItems => m.equipment[0].is_some(),
            AGoal::FollowRidden { following, .. } => {
                (*following && self.player_moved()) || self.follow_ridden_can_use(m)
            }
            _ => false,
        }
    }

    pub(crate) fn dolphin_can_continue(&mut self, g: &mut AGoal, m: &mut Mob) -> bool {
        match g {
            AGoal::Treasure { stuck } => {
                let Some(t) = m.sea.treasure else { return false };
                let c = [f64::from(t[0]) + 0.5, m.pos[1].floor() + 0.5, f64::from(t[2]) + 0.5];
                dist2(c, m.pos) >= 16.0 && !*stuck && m.air >= 100
            }
            AGoal::SwimWithPlayer { on } => {
                *on && self.player_swimming() && dist2(m.pos, self.pos) < 256.0 && self.ent_alive(Ent::Player)
            }
            AGoal::Jump { .. } => {
                let yd = m.vel[1];
                (yd * yd >= f64::from(0.03f32) || m.pitch == 0.0 || m.pitch.abs() >= 10.0 || !m.in_water)
                    && !m.on_ground
            }
            AGoal::FollowRidden { following, .. } => self.follow_ridden_can_continue(*following),
            _ => self.dolphin_can_use(g, m),
        }
    }

    pub(crate) fn dolphin_start(&mut self, g: &mut AGoal, m: &mut Mob) {
        match g {
            AGoal::BreathAir => {
                m.nav.stop();
                self.find_air_position(m);
            }
            AGoal::FindLiquid => {
                let b = m.block_pos();
                let lo = [(m.pos[0] - 2.0).floor() as i32, (m.pos[1] - 2.0).floor() as i32, (m.pos[2] - 2.0).floor() as i32];
                let hi = [(m.pos[0] + 2.0).floor() as i32, b[1], (m.pos[2] + 2.0).floor() as i32];
                'z: for z in lo[2]..=hi[2] {
                    for y in lo[1]..=hi[1] {
                        for x in lo[0]..=hi[0] {
                            if self.mob_source_water([x, y, z]) {
                                super::super::nav::set_wanted(m, [f64::from(x), f64::from(y), f64::from(z)], 1.0);
                                break 'z;
                            }
                        }
                    }
                }
            }
            AGoal::Treasure { stuck } => {
                *stuck = false;
                m.nav.stop();
                match self.nearest_structure("minecraft:dolphin_located", m.block_pos(), 50) {
                    Some(p) => {
                        m.sea.treasure = Some(p);
                        self.mob_event(m, 38);
                    }
                    None => *stuck = true,
                }
            }
            AGoal::SwimWithPlayer { .. } => {
                self.add_effect_from(
                    crate::effects::Inst::new("minecraft:dolphins_grace", 100, 0),
                    Some(crate::progress::pred::Who::Mob(m)),
                );
            }
            AGoal::Jump { .. } => {
                let (sx, sz) = facing(m.yaw);
                m.vel[0] += f64::from(sx) * 0.6;
                m.vel[1] += 0.7;
                m.vel[2] += f64::from(sz) * 0.6;
                m.nav.stop();
            }
            AGoal::MoveToItem { cooldown } => {
                if let Some(&(_, p)) = items_near(self, m).first() {
                    self.move_to_entity(m, p, f64::from(1.2f32));
                    self.mob_sound(m, "minecraft:entity.dolphin.play", 1.0, 1.0);
                }
                *cooldown = 0;
            }
            AGoal::FollowRidden { following, recalc, toward } => {
                *following = self.ridden_nautilus_near(m);
                *recalc = 0;
                *toward = false;
            }
            _ => {}
        }
    }

    pub(crate) fn dolphin_stop(&mut self, g: &mut AGoal, m: &mut Mob) {
        match g {
            AGoal::Treasure { stuck } => {
                let close = m.sea.treasure.is_some_and(|t| {
                    let c = [f64::from(t[0]) + 0.5, m.pos[1].floor() + 0.5, f64::from(t[2]) + 0.5];
                    dist2(c, m.pos) < 16.0
                });
                if m.sea.treasure.is_none() || close || *stuck {
                    m.sea.got_fish = false;
                }
            }
            AGoal::SwimWithPlayer { on } => {
                *on = false;
                m.nav.stop();
            }
            AGoal::Jump { .. } => m.pitch = 0.0,
            AGoal::MoveToItem { cooldown } => {
                if self.dolphin_drop_item(m) {
                    *cooldown = m.tick_count + m.rng.next_int_bound(100);
                }
            }
            AGoal::PlayWithItems => {
                self.dolphin_drop_item(m);
            }
            AGoal::FollowRidden { following, .. } => *following = false,
            _ => {}
        }
    }

    pub(crate) fn dolphin_tick(&mut self, g: &mut AGoal, m: &mut Mob) {
        match g {
            AGoal::BreathAir => {
                self.find_air_position(m);
                self.move_relative_pub(m, 0.02);
                let v = m.vel;
                self.mob_move(m, v);
            }
            AGoal::Treasure { stuck } => self.treasure_tick(m, stuck),
            AGoal::SwimWithPlayer { .. } => {
                let eye = self.player_eye_y();
                self.look_at(m, [self.pos[0], eye, self.pos[2]], 21.0, 1.0);
                if dist2(m.pos, self.pos) < 6.25 {
                    m.nav.stop();
                } else {
                    let p = self.pos;
                    self.move_to_entity(m, p, 4.0);
                }
                if self.player_swimming() && self.mobs.rng.next_int_bound(6) == 0 {
                    self.add_effect_from(
                    crate::effects::Inst::new("minecraft:dolphins_grace", 100, 0),
                    Some(crate::progress::pred::Who::Mob(m)),
                );
                }
            }
            AGoal::Jump { breached, .. } => {
                // `breached` is never reset to false: the sound plays only on the first jump.
                let already = *breached;
                if !already {
                    *breached = self.mob_fluid(m.block_pos()) == super::super::env::Fluid::Water;
                }
                if *breached && !already {
                    self.mob_sound(m, "minecraft:entity.dolphin.jump", 1.0, 1.0);
                }
                let v = m.vel;
                if v[1] * v[1] < f64::from(0.03f32) && m.pitch != 0.0 {
                    m.pitch += 0.2 * wrap(0.0 - m.pitch);
                } else if (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() > f64::from(1.0e-5f32) {
                    let h = (v[0] * v[0] + v[2] * v[2]).sqrt();
                    m.pitch = ((-v[1]).atan2(h) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32;
                }
            }
            AGoal::MoveToItem { .. } => {
                if let Some(&(_, p)) = items_near(self, m).first() {
                    self.move_to_entity(m, p, f64::from(1.2f32));
                }
            }
            AGoal::PlayWithItems => {
                self.dolphin_drop_item(m);
            }
            AGoal::FollowRidden { recalc, toward, .. } => self.follow_ridden_tick(m, recalc, toward),
            _ => {}
        }
    }

    /// Column and neighbors up to 8 blocks higher.
    fn find_air_position(&mut self, m: &mut Mob) {
        let b = m.block_pos();
        let mut dest = None;
        'search: for (dx, dz) in [(0, 0), (0, -1), (1, 0), (0, 1), (-1, 0)] {
            for y in b[1]..=b[1] + 8 {
                let p = [b[0] + dx, y, b[2] + dz];
                let empty = self.mob_fluid(p) == super::super::env::Fluid::None;
                if (empty || self.bname(p) == "minecraft:bubble_column") && self.pathfindable_land(p) {
                    dest = Some(p);
                    break 'search;
                }
            }
        }
        let d = dest.unwrap_or_else(|| super::super::mth::block_of([m.pos[0], m.pos[1] + 8.0, m.pos[2]]));
        self.move_to_pos(m, [f64::from(d[0]), f64::from(d[1] + 1), f64::from(d[2])], 1.0);
    }

    fn treasure_tick(&mut self, m: &mut Mob, stuck: &mut bool) {
        let Some(t) = m.sea.treasure else { return };
        let close_next = m.nav.target_pos.is_some_and(|p| {
            dist2([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5], m.pos) < 144.0
        });
        if !(close_next || m.nav.is_done()) {
            return;
        }
        let c = [f64::from(t[0]) + 0.5, f64::from(t[1]) + 0.5, f64::from(t[2]) + 0.5];
        let mut next = self.pos_towards(m, 16, 1, c, std::f32::consts::PI / 8.0);
        if next.is_none() {
            next = self.pos_towards(m, 8, 4, c, std::f32::consts::FRAC_PI_2);
        }
        if let Some(n) = next {
            let q = super::super::mth::block_of(n);
            if self.mob_fluid(q) != super::super::env::Fluid::Water {
                next = self.pos_towards(m, 8, 5, c, std::f32::consts::FRAC_PI_2);
            }
        }
        let Some(n) = next else {
            *stuck = true;
            return;
        };
        self.look_at(m, n, 21.0, 1.0);
        self.move_to_pos(m, n, f64::from(1.3f32));
        if self.mobs.rng.next_int_bound(super::super::mth::reduced(80)) == 0 {
            self.mob_event(m, 38);
        }
    }

    /// Has the player moved horizontally?
    fn player_moved(&self) -> bool {
        let v = self.mobs.cubes.player_speed;
        (v[0] * v[0] + v[2] * v[2]).sqrt() > f64::from(1.0e-5f32)
    }

    /// Nautilus steered by the player in the box inflated by 5.
    fn ridden_nautilus_near(&self, m: &Mob) -> bool {
        let Some(v) = self.mobs.mounts.riding.vehicle else { return false };
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|x| x - 5.0), max: b.max.map(|x| x + 5.0) };
        self.mob_ref(v).is_some_and(|o| super::is_nautilus(o.kind) && Self::player_controls(o) && o.touches(&area))
    }

    /// The start condition of following a player's mount (boats not simulated: only nautiluses).
    fn follow_ridden_can_use(&self, m: &Mob) -> bool {
        self.player_moved() && self.ridden_nautilus_near(m)
    }

    fn follow_ridden_can_continue(&self, following: bool) -> bool {
        following && self.mobs.mounts.riding.vehicle.is_some() && self.player_moved()
    }

    /// Behind the player, then in their movement direction at 10 blocks; small push
    /// of its own at each tick.
    fn follow_ridden_tick(&mut self, m: &mut Mob, recalc: &mut i32, toward: &mut bool) {
        let speed = if *toward { 0.01 } else { 0.015 };
        self.move_relative_pub(m, speed);
        let v = m.vel;
        self.mob_move(m, v);
        *recalc -= 1;
        if *recalc > 0 {
            return;
        }
        *recalc = super::super::mth::reduced(10);
        let pb = super::super::mth::block_of(self.pos);
        let d = dist2(m.pos, self.pos).sqrt();
        if !*toward {
            // Horizontal direction of the player, opposite.
            let (dx, dz) = facing(self.yaw);
            let behind = [pb[0] - dx, pb[1] - 1, pb[2] - dz];
            self.move_to_pos(m, [f64::from(behind[0]), f64::from(behind[1]), f64::from(behind[2])], 1.0);
            if d < 4.0 {
                *recalc = 0;
                *toward = true;
            }
        } else {
            // The player's "movement" direction = that of their yaw (never vertical).
            let (sx, sz) = facing(self.yaw);
            let to = [pb[0] + sx * 10, pb[1] - 1, pb[2] + sz * 10];
            self.move_to_pos(m, [f64::from(to[0]), f64::from(to[1]), f64::from(to[2])], 1.0);
            if d > 12.0 {
                *recalc = 0;
                *toward = false;
            }
        }
    }
}
