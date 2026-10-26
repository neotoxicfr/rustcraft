//! Phantom (family F01): own flight (movement control with internal speed, movement
//! without gravity), goals (circle / swoop strategy, swoop, circle around the anchor, targeting of the
//! highest player), spawner tied to sleep deprivation and the player's "time since
//! rest" statistic.

use super::mob::{Ent, Mob};
use super::mth::{DEG, atan2, cos, degrees_difference, dist2, reduced, sin, wrap};
use super::spawn::{Group, Reason};
use super::species::Kind;
use crate::server::Server;

fn approach(cur: f32, target: f32, inc: f32) -> f32 {
    let inc = inc.abs();
    if cur < target { (cur + inc).clamp(cur, target) } else { (cur - inc).clamp(target, cur) }
}

fn approach_degrees(cur: f32, target: f32, inc: f32) -> f32 {
    let d = degrees_difference(cur, target);
    approach(cur, cur + d, inc)
}

impl Server {
    fn sea_level(&self) -> i32 {
        if self.flat() { -63 } else { 63 }
    }

    /// Attackable target (combat, line of sight).
    pub(crate) fn phantom_can_attack(&self, m: &mut Mob) -> bool {
        match m.target {
            Some(t) => self.targeting_ok(m, t, true, -1.0, true, true),
            None => false,
        }
    }

    /// Targeting: sweep every ~60 ticks of players within ±16 / ±64, the highest first.
    pub(crate) fn phantom_target_can_use(&self, m: &mut Mob, next_scan: &mut i32) -> bool {
        if *next_scan > 0 {
            *next_scan -= 1;
            return false;
        }
        *next_scan = reduced(60);
        let b = m.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - 16.0, b.min[1] - 64.0, b.min[2] - 16.0],
            max: [b.max[0] + 16.0, b.max[1] + 64.0, b.max[2] + 16.0],
        };
        // A single player in singleplayer: the list sorted by height reduces to them.
        if self.player_aabb().intersects(&area)
            && self.targeting_ok(m, Ent::Player, true, 64.0, true, true)
            && self.targeting_ok(m, Ent::Player, true, -1.0, true, true)
        {
            m.target = Some(Ent::Player);
            return true;
        }
        false
    }

    pub(crate) fn phantom_anchor_above_target(&self, m: &mut Mob) {
        if m.x.anchor.is_none() {
            return;
        }
        let Some(tp) = m.target.and_then(|t| self.ent_pos(t)) else { return };
        let b = super::mth::block_of(tp);
        let mut a = [b[0], b[1] + 20 + m.rng.next_int_bound(20), b[2]];
        if a[1] < self.sea_level() {
            a[1] = self.sea_level() + 1;
        }
        m.x.anchor = Some(a);
    }

    pub(crate) fn phantom_strategy_stop(&self, m: &mut Mob) {
        if let Some(a) = m.x.anchor {
            let top = self.surface_y(a[0], a[2], 4);
            m.x.anchor = Some([a[0], top + 10 + m.rng.next_int_bound(20), a[2]]);
        }
    }

    pub(crate) fn phantom_strategy_tick(&mut self, m: &mut Mob, next_sweep: &mut i32) {
        if !m.x.swoop {
            *next_sweep -= 1;
            if *next_sweep <= 0 {
                m.x.swoop = true;
                self.phantom_anchor_above_target(m);
                *next_sweep = reduced((8 + m.rng.next_int_bound(4)) * 20);
                let p = 0.95 + m.rng.next_float() * 0.1;
                self.mob_sound(m, "minecraft:entity.phantom.swoop", 10.0, p);
            }
        }
    }

    /// The swoop continues while the target lives, is not creative, and no cat is nearby.
    pub(crate) fn phantom_sweep_continue(&mut self, m: &mut Mob, scared: &mut bool, cat_tick: &mut i32) -> bool {
        let Some(t) = m.target else { return false };
        if !self.ent_alive(t) {
            return false;
        }
        if t == Ent::Player && (self.game_mode == 1 || self.game_mode == 3) {
            return false;
        }
        if !m.x.swoop {
            return false;
        }
        if m.tick_count > *cat_tick {
            *cat_tick = m.tick_count + 20;
            let b = m.bbox();
            let area = rc_physics::Aabb { min: b.min.map(|v| v - 16.0), max: b.max.map(|v| v + 16.0) };
            let cats: Vec<i32> = self
                .mobs
                .list
                .iter()
                .flatten()
                .filter(|o| o.kind == Kind::Cat && o.alive() && o.touches(&area))
                .map(|o| o.id)
                .collect();
            for id in &cats {
                if let Some(i) = self.mob_index(*id) {
                    let mut c = self.mobs.list[i].take().expect("chat");
                    let hiss =
                        self.variant_sound(&c, "hiss_sound").unwrap_or_else(|| "minecraft:entity.cat.hiss".into());
                    self.make_sound(&mut c, &hiss);
                    self.mobs.list[i] = Some(c);
                }
            }
            *scared = !cats.is_empty();
        }
        !*scared
    }

    pub(crate) fn phantom_sweep_tick(&mut self, m: &mut Mob) {
        let Some(t) = m.target else { return };
        let (Some(tp), Some(tb)) = (self.ent_pos(t), self.ent_bbox(t)) else { return };
        m.x.move_target = [tp[0], tp[1] + (tb.max[1] - tb.min[1]) * 0.5, tp[2]];
        let b = m.bbox();
        let g = f64::from(0.2f32);
        let grown = rc_physics::Aabb { min: b.min.map(|v| v - g), max: b.max.map(|v| v + g) };
        if grown.intersects(&tb) {
            self.mob_attack(m, t);
            m.x.swoop = false;
            let _ = self.send(rc_client::Event::LevelEvent { kind: 1039, pos: m.block_pos(), data: 0, global: false });
        } else if m.horizontal_collision || m.hurt_time > 0 {
            m.x.swoop = false;
        }
    }

    pub(crate) fn phantom_select_next(&self, m: &mut Mob, angle: &mut f32, dist: f32, height: f32, cw: f32) {
        if m.x.anchor.is_none() {
            m.x.anchor = Some(m.block_pos());
        }
        let a = m.x.anchor.unwrap_or_default();
        *angle += cw * 15.0 * DEG;
        m.x.move_target = [
            f64::from(a[0]) + f64::from(dist * cos(*angle)),
            f64::from(a[1]) + f64::from(-4.0 + height),
            f64::from(a[2]) + f64::from(dist * sin(*angle)),
        ];
    }

    pub(crate) fn phantom_circle_tick(
        &mut self,
        m: &mut Mob,
        angle: &mut f32,
        dist: &mut f32,
        height: &mut f32,
        cw: &mut f32,
    ) {
        if m.rng.next_int_bound(reduced(350)) == 0 {
            *height = -4.0 + m.rng.next_float() * 9.0;
        }
        if m.rng.next_int_bound(reduced(250)) == 0 {
            *dist += 1.0;
            if *dist > 15.0 {
                *dist = 5.0;
                *cw = -*cw;
            }
        }
        if m.rng.next_int_bound(reduced(450)) == 0 {
            *angle = m.rng.next_float() * 2.0 * std::f32::consts::PI;
            self.phantom_select_next(m, angle, *dist, *height, *cw);
        }
        if dist2(m.x.move_target, m.pos) < 4.0 {
            self.phantom_select_next(m, angle, *dist, *height, *cw);
        }
        let bp = m.block_pos();
        if m.x.move_target[1] < m.pos[1] && !self.is_air([bp[0], bp[1] - 1, bp[2]]) {
            *height = height.max(1.0);
            self.phantom_select_next(m, angle, *dist, *height, *cw);
        }
        if m.x.move_target[1] > m.pos[1] && !self.is_air([bp[0], bp[1] + 1, bp[2]]) {
            *height = height.min(-1.0);
            self.phantom_select_next(m, angle, *dist, *height, *cw);
        }
    }

    /// Flight control: yaw bounded to 4°/tick, internal speed 0.1 → 1.8, momentum smoothed at 20 %.
    pub(crate) fn phantom_move_control(&self, m: &mut Mob) {
        if m.horizontal_collision {
            m.yaw += 180.0;
            m.x.fly_speed = 0.1;
        }
        let t = m.x.move_target;
        let mut tdx = t[0] - m.pos[0];
        let tdy = t[1] - m.pos[1];
        let mut tdz = t[2] - m.pos[2];
        let mut sd = (tdx * tdx + tdz * tdz).sqrt();
        if sd.abs() <= f64::from(1.0e-5f32) {
            return;
        }
        let scale = 1.0 - (tdy * f64::from(0.7f32)).abs() / sd;
        tdx *= scale;
        tdz *= scale;
        sd = (tdx * tdx + tdz * tdz).sqrt();
        let sd2 = (tdx * tdx + tdz * tdz + tdy * tdy).sqrt();
        let prev = m.yaw;
        let ang = atan2(tdz, tdx) as f32;
        let a = wrap(m.yaw + 90.0);
        let b = wrap(ang * (180.0 / std::f32::consts::PI));
        m.yaw = approach_degrees(a, b, 4.0) - 90.0;
        m.body_yaw = m.yaw;
        let s = m.x.fly_speed;
        m.x.fly_speed = if degrees_difference(prev, m.yaw).abs() < 3.0 {
            approach(s, 1.8, 0.005 * (1.8 / s))
        } else {
            approach(s, 0.2, 0.025)
        };
        let xr = -((atan2(-tdy, sd) * f64::from(180.0f32 / std::f32::consts::PI)) as f32);
        m.pitch = xr;
        let mv = m.yaw + 90.0;
        let sp = f64::from(m.x.fly_speed);
        let txd = sp * f64::from(cos(mv * DEG)) * (tdx / sd2).abs();
        let tzd = sp * f64::from(sin(mv * DEG)) * (tdz / sd2).abs();
        let tyd = sp * f64::from(sin(xr * DEG)) * (tdy / sd2).abs();
        let v = m.vel;
        m.vel = [v[0] + (txd - v[0]) * 0.2, v[1] + (tyd - v[1]) * 0.2, v[2] + (tzd - v[2]) * 0.2];
    }

    /// Water 0.8, lava 0.5, air 0.91, no gravity.
    pub(crate) fn phantom_travel(&mut self, m: &mut Mob) {
        let (speed, drag) = if m.in_water {
            (0.02f32, f64::from(0.8f32))
        } else if m.lava_height > 0.0 {
            (0.02, 0.5)
        } else {
            (0.2, f64::from(0.91f32))
        };
        let input = [f64::from(m.xxa), f64::from(m.yya), f64::from(m.zza)];
        let len = input[0] * input[0] + input[1] * input[1] + input[2] * input[2];
        if len >= 1.0e-7 {
            let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
            let v = v.map(|x| x * f64::from(speed));
            let (s, c) = (f64::from(sin(m.yaw * DEG)), f64::from(cos(m.yaw * DEG)));
            m.vel[0] += v[0] * c - v[2] * s;
            m.vel[1] += v[1];
            m.vel[2] += v[2] * c + v[0] * s;
        }
        let v = m.vel;
        self.mob_move(m, v);
        m.vel = m.vel.map(|x| x * drag);
    }

    // ---------- Spawning by insomnia ----------

    /// Tick of the phantom spawner. "Time since rest": counter
    /// of the player's statistics (S9: +1 per tick out of bed, reset on sleeping and on
    /// death, saved in `players/stats/`).
    pub(crate) fn phantom_spawner_tick(&mut self) {
        let spawn_enemies = self.level.difficulty_id() != 0 && self.rule("minecraft:spawn_monsters", true);
        if !spawn_enemies || !self.rule("minecraft:spawn_phantoms", true) {
            return;
        }
        self.mobs.phantom_next -= 1;
        if self.mobs.phantom_next > 0 {
            return;
        }
        self.mobs.phantom_next += (60 + self.mobs.rng.next_int_bound(60)) * 20;
        // The spawner exists only in the Overworld (which has sky light): the countdown
        // continues there, but a player in the Nether or the End is never a candidate there.
        if self.dim.name != "minecraft:overworld" || self.mob_sky_darken() < 5 {
            return;
        }
        // Every player of the world is a candidate, in list order (host, then guests
        // joined), with their own position, game mode and time since rest.
        self.phantom_try_player();
        let n = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..n {
            if self.lan.as_ref().is_some_and(|l| l.guests[i].joined && !l.guests[i].gone) {
                self.as_guest(i, |sv| sv.phantom_try_player());
            }
        }
    }

    /// Spawner draws for the current player (non-spectator).
    fn phantom_try_player(&mut self) {
        if self.game_mode == 3 {
            return;
        }
        let rest = self.stat_value("minecraft:custom", "minecraft:time_since_rest");
        let pp = super::mth::block_of(self.pos);
        if pp[1] < self.sea_level() || !self.can_see_sky(pp) {
            return;
        }
        let (eff, _) = self.local_difficulty(pp);
        if eff <= self.mobs.rng.next_float() * 3.0 {
            return;
        }
        let value = rest.max(1);
        if self.mobs.rng.next_int_bound(value) < 72000 {
            return;
        }
        let up = 20 + self.mobs.rng.next_int_bound(15);
        let east = -10 + self.mobs.rng.next_int_bound(21);
        let south = -10 + self.mobs.rng.next_int_bound(21);
        let sp = [pp[0] + east, pp[1] + up, pp[2] + south];
        if !self.phantom_empty_spawn(sp) {
            return;
        }
        let n = 1 + self.mobs.rng.next_int_bound(self.level.difficulty_id() + 1);
        let mut group = Group::default();
        for _ in 0..n {
            let pos = [f64::from(sp[0]) + 0.5, f64::from(sp[1]), f64::from(sp[2]) + 0.5];
            let mut m = self.new_mob(Kind::Phantom, pos, 0.0);
            m.head_yaw = 0.0;
            m.body_yaw = 0.0;
            self.finalize_spawn(&mut m, Reason::Natural, &mut group);
            self.add_mob(m);
        }
    }

    /// The valid empty spawn block test for the phantom.
    fn phantom_empty_spawn(&self, p: [i32; 3]) -> bool {
        self.valid_empty_spawn(p, Kind::Phantom)
    }
}
