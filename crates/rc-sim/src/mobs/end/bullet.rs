//! Shulker bullet (F15) on the common projectile engine (S8, `PKind::ShulkerBullet`):
//! advances in segments along the axes (10 to 50 ticks each, never on the attach axis at the
//! start), target speed 0.15 growing by 2.5% per tick, gravity 0.04 without a living target;
//! 4 damage and levitation 10 s on impact; destroyed by a block, a hit or an explosion.

use rc_proto::nbt::Nbt;

use super::super::mob::{Ent, Mob};
use super::super::mth::{atan2, block_of, dist2};
use crate::blocks::Dir;
use crate::effects::Inst;
use crate::nbtio::int;
use crate::player::Stack;
use crate::projectiles::physics::Hit;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

/// The bullet's own state (final target, current movement direction, flight steps, offset from the target).
#[derive(Debug, Clone, Default)]
pub struct BulletState {
    pub target: Option<u128>,
    /// Direction of the current segment (index 0..6: down, up, north, south, west, east).
    pub dir: Option<i32>,
    pub steps: i32,
    pub td: [f64; 3],
    /// Position at the start of the previous tick (not saved: nothing on the first tick after loading).
    pub last_known: Option<[f64; 3]>,
    /// Known velocity: movement between the last two tick starts (momentum deflection).
    pub known_speed: [f64; 3],
}

impl BulletState {
    pub fn load(e: &Nbt) -> BulletState {
        let f = |k: &str| match e.get(k) {
            Some(Nbt::Double(v)) => *v,
            _ => 0.0,
        };
        BulletState {
            target: crate::nbtio::uuid(e.get("Target")),
            dir: int(e, "Dir").and_then(|v| (0..6).contains(&v).then_some(v as i32)),
            steps: int(e, "Steps").unwrap_or(0) as i32,
            td: [f("TXD"), f("TYD"), f("TZD")],
            ..Default::default()
        }
    }

    /// Own fields, in the game's order.
    pub fn save(&self, f: &mut Vec<(String, Nbt)>) {
        if let Some(t) = self.target {
            f.push(("Target".into(), crate::nbtio::uuid_nbt(t)));
        }
        if let Some(d) = self.dir {
            f.push(("Dir".into(), Nbt::Byte(d as i8)));
        }
        f.push(("Steps".into(), Nbt::Int(self.steps)));
        f.push(("TXD".into(), Nbt::Double(self.td[0])));
        f.push(("TYD".into(), Nbt::Double(self.td[1])));
        f.push(("TZD".into(), Nbt::Double(self.td[2])));
    }
}

/// Axis of a direction (0: Y, 1: Z, 2: X), like the attach index / 2.
fn axis(d: i32) -> i32 {
    d / 2
}

impl Server {
    /// New bullet (`ShulkerBullet(level, shulker, target, forbidden axis)`): at the center of the
    /// shulker's box, initial direction "up", first segment chosen immediately.
    pub(crate) fn shoot_bullet(&mut self, owner: &Mob, target: Ent, avoid_axis: i32) {
        let id = self.proj_new_id();
        let b = super::shulker::bbox(owner);
        let pos = std::array::from_fn(|k| (b.min[k] + b.max[k]) / 2.0);
        let mut p =
            Proj::new(PKind::ShulkerBullet, id, pos, Stack { id: "minecraft:air".into(), count: 1, components: None });
        p.owner = Some(owner.uuid);
        let target_uuid = match target {
            Ent::Player => Some(self.uuid),
            Ent::Mob(i) => self.mob_ref(i).map(|o| o.uuid),
        };
        let mut st = BulletState { target: target_uuid, dir: Some(1), ..Default::default() };
        self.bullet_next_direction(&mut p, &mut st, Some(avoid_axis), Some(target));
        p.bullet = Some(Box::new(st));
        self.proj_add(p);
    }

    /// Target resolved by UUID (the player even dead, or a mob present).
    fn bullet_target(&self, st: &BulletState) -> Option<Ent> {
        let u = st.target?;
        if u == self.uuid {
            return Some(Ent::Player);
        }
        self.mobs.find_uuid_where(u, |o| !o.removed).map(|o| Ent::Mob(o.id))
    }

    /// Toward the target along an allowed axis (free cell), otherwise at random.
    fn bullet_next_direction(&mut self, p: &mut Proj, st: &mut BulletState, avoid: Option<i32>, target: Option<Ent>) {
        let mut y_off = 0.5;
        let tp = match target.and_then(|t| self.ent_pos(t).zip(self.ent_bbox(t))) {
            None => {
                let b = block_of(p.pos);
                [b[0], b[1] - 1, b[2]]
            }
            Some((q, bb)) => {
                y_off = (bb.max[1] - bb.min[1]) * 0.5;
                block_of([q[0], q[1] + y_off, q[2]])
            }
        };
        let mut tx = f64::from(tp[0]) + 0.5;
        let mut ty = f64::from(tp[1]) + y_off;
        let mut tz = f64::from(tp[2]) + 0.5;
        let mut selection = None;
        let center = [f64::from(tp[0]) + 0.5, f64::from(tp[1]) + 0.5, f64::from(tp[2]) + 0.5];
        if dist2(center, p.pos) >= 4.0 {
            let cur = block_of(p.pos);
            let empty = |s: &Server, d: Dir| {
                let o = d.step();
                s.is_air([cur[0] + o[0], cur[1] + o[1], cur[2] + o[2]])
            };
            let mut options: Vec<i32> = Vec::new();
            if avoid != Some(2) {
                if cur[0] < tp[0] && empty(self, Dir::East) {
                    options.push(5);
                } else if cur[0] > tp[0] && empty(self, Dir::West) {
                    options.push(4);
                }
            }
            if avoid != Some(0) {
                if cur[1] < tp[1] && empty(self, Dir::Up) {
                    options.push(1);
                } else if cur[1] > tp[1] && empty(self, Dir::Down) {
                    options.push(0);
                }
            }
            if avoid != Some(1) {
                if cur[2] < tp[2] && empty(self, Dir::South) {
                    options.push(3);
                } else if cur[2] > tp[2] && empty(self, Dir::North) {
                    options.push(2);
                }
            }
            // The direction draw happens even when an option exists (draw order).
            let mut sel = p.rng.next_int_bound(6);
            if options.is_empty() {
                let mut attempts = 5;
                while !empty(self, Dir::from_index(sel as usize)) && attempts > 0 {
                    sel = p.rng.next_int_bound(6);
                    attempts -= 1;
                }
            } else {
                sel = options[p.rng.next_int_bound(options.len() as i32) as usize];
            }
            let o = Dir::from_index(sel as usize).step();
            tx = p.pos[0] + f64::from(o[0]);
            ty = p.pos[1] + f64::from(o[1]);
            tz = p.pos[2] + f64::from(o[2]);
            selection = Some(sel);
        }
        st.dir = selection;
        let (xa, ya, za) = (tx - p.pos[0], ty - p.pos[1], tz - p.pos[2]);
        let d = (xa * xa + ya * ya + za * za).sqrt();
        st.td = if d == 0.0 { [0.0; 3] } else { [xa / d * 0.15, ya / d * 0.15, za / d * 0.15] };
        p.needs_sync = true;
        st.steps = 10 + p.rng.next_int_bound(5) * 10;
    }

    /// The tick of a shulker projectile (preceded by the despawn test: removed in peaceful).
    pub(crate) fn shulker_bullet_tick(&mut self, p: &mut Proj) {
        if self.level.difficulty_id() == 0 {
            p.removed = true;
            return;
        }
        // State kept in its box (no allocation per tick).
        let mut st = p.bullet.take().unwrap_or_default();
        // Known velocity, taken at the start of the tick like the game's engine.
        let last = st.last_known.unwrap_or(p.pos);
        st.known_speed = [p.pos[0] - last[0], p.pos[1] - last[1], p.pos[2] - last[2]];
        st.last_known = Some(p.pos);
        self.proj_super_tick(p, true);
        if p.removed {
            p.bullet = Some(st);
            return;
        }
        let target = self.bullet_target(&st);
        if target.is_none() {
            st.target = None;
        }
        let live = target.is_some_and(|t| self.ent_alive(t) && !(t == Ent::Player && self.game_mode == 3));
        if !live {
            if !p.no_gravity {
                p.vel[1] -= PKind::ShulkerBullet.gravity();
            }
        } else {
            for k in 0..3 {
                st.td[k] = (st.td[k] * 1.025).clamp(-1.0, 1.0);
                p.vel[k] += (st.td[k] - p.vel[k]) * 0.2;
            }
        }
        let hit = self.hit_on_move(p, false);
        let mv = p.vel;
        p.pos = [p.pos[0] + mv[0], p.pos[1] + mv[1], p.pos[2] + mv[2]];
        self.proj_blocks_effects(p);
        if !matches!(hit, Hit::Miss(_)) && !p.removed {
            // Put back in place during the impact: the momentum deflection reads the known velocity.
            p.bullet = Some(st);
            self.proj_hit(p, hit);
            st = p.bullet.take().unwrap_or_default();
        }
        // The rotation toward the movement.
        let v = p.vel;
        if v != [0.0; 3] {
            let h = (v[0] * v[0] + v[2] * v[2]).sqrt();
            let deg = f64::from(180.0f32 / std::f32::consts::PI);
            let yr = (atan2(v[2], v[0]) * deg) as f32 + 90.0;
            let xr = (atan2(h, v[1]) * deg) as f32 - 90.0;
            p.pitch = lerp(p.pitch, xr);
            p.yaw = lerp(p.yaw, yr);
        }
        if let Some(t) = target {
            if st.steps > 0 {
                st.steps -= 1;
                if st.steps == 0 {
                    let a = st.dir.map(axis);
                    self.bullet_next_direction(p, &mut st, a, Some(t));
                }
            }
            if let Some(d) = st.dir {
                let cur = block_of(p.pos);
                let o = Dir::from_index(d as usize).step();
                let next = [cur[0] + o[0], cur[1] + o[1], cur[2] + o[2]];
                let a = axis(d);
                let loaded = self.chunks.get(&(next[0] >> 4, next[2] >> 4)).is_some_and(Option::is_some);
                if loaded && self.face_full_up(self.get(next)) {
                    self.bullet_next_direction(p, &mut st, Some(a), Some(t));
                } else if let Some(tp) = self.ent_pos(t) {
                    let tb = block_of(tp);
                    if (a == 2 && cur[0] == tb[0]) || (a == 1 && cur[2] == tb[2]) || (a == 0 && cur[1] == tb[1]) {
                        self.bullet_next_direction(p, &mut st, Some(a), Some(t));
                    }
                }
            }
        }
        p.bullet = Some(st);
    }

    /// 4 damage (mob projectile) then levitation 200 ticks.
    pub(crate) fn shulker_bullet_hit(&mut self, p: &Proj, t: Ent, owner: Option<Ent>) {
        // Direct source = the bullet (the shulker hit teleports or duplicates).
        self.mobs.end.direct = Some("minecraft:shulker_bullet");
        let ok = self.proj_damage(t, "minecraft:mob_projectile", 4.0, owner, (-p.vel[0], -p.vel[2]));
        self.mobs.end.direct = None;
        // A part of the dragon is not living (no effect).
        if ok && self.target_kind(t) != Some(super::super::species::Kind::EnderDragon) {
            self.give_ent_effect(t, Inst::new("minecraft:levitation", 200, 0), 1.0);
        }
    }

    /// Bullet hit (player hit, arrow, explosion): sound, 15 particles, destroyed; true if
    /// `id` was a living bullet.
    pub(crate) fn bullet_destroy(&mut self, id: i32) -> bool {
        let Some(p) = self.proj.list.iter_mut().find(|p| p.id == id && !p.removed && p.kind == PKind::ShulkerBullet)
        else {
            return false;
        };
        p.removed = true;
        let pos = p.pos;
        self.world_sound("minecraft:entity.shulker_bullet.hurt", 5, pos, 1.0, 1.0);
        self.particles("minecraft:crit", pos, [0.2, 0.2, 0.2], 0.0, 15);
        true
    }

    /// Player hit on a bullet; true if it was a bullet.
    pub(crate) fn bullet_attacked(&mut self, id: i32) -> bool {
        if !self.proj.list.iter().any(|p| p.id == id && !p.removed && p.kind == PKind::ShulkerBullet) {
            return false;
        }
        if self.game_mode == 3 || self.life.vit.dead {
            return true;
        }
        self.mobs.attack_ticker = 0;
        self.bullet_destroy(id);
        // Non-living target hit: the player forgets their last victim.
        self.set_last_hurt_mob(Some(Ent::Player), None);
        true
    }
}

fn lerp(mut old: f32, new: f32) -> f32 {
    while new - old < -180.0 {
        old -= 360.0;
    }
    while new - old >= 180.0 {
        old += 360.0;
    }
    old + (new - old) * 0.5
}
