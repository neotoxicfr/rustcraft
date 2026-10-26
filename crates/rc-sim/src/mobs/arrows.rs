//! Bow shots by mobs through the common engine of the
//! projectiles (`projectiles/`, S8): arrow created by the mob (no pickup), base damage
//! drawn on the arrow's random, variant effect (stray, bogged, parched), burning arrow
//! of the wither skeleton, aimed shot (inaccuracy according to difficulty). Flight, impacts,
//! network tracking and saving are those of the player's arrows.
//! Also: clipping of a segment by blocks and boxes (`clip_*`), shared by mobs.

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::mob::{Ent, Mob};
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

/// Held ammunition: off hand then main hand, among arrows.
fn held_arrow(m: &Mob) -> Option<Stack> {
    let ok =
        |s: &&Stack| matches!(s.id.as_str(), "minecraft:arrow" | "minecraft:tipped_arrow" | "minecraft:spectral_arrow");
    [1usize, 0].iter().find_map(|&h| m.equipment[h].as_ref().filter(ok)).cloned()
}

impl Server {
    /// Arrow of a mob (the creation of an item's arrow + constructor of the base arrow):
    /// at eye level − 0.1, owner the mob, pickup forbidden (except `intangible_projectile`:
    /// creative only).
    pub(crate) fn mob_create_arrow(&mut self, m: &Mob, ammo: &Stack, weapon: Option<&Stack>) -> Proj {
        let pos = [m.pos[0], m.eye_y() - f64::from(0.1f32), m.pos[2]];
        let mut p = self.new_arrow(pos, ammo, weapon);
        p.owner = Some(m.uuid);
        p
    }

    /// Arrow shot by a mob (bow power).
    pub(crate) fn shoot_arrow(&mut self, m: &mut Mob, target: Ent, power: f32) {
        let (Some(tp), Some(tb)) = (self.ent_pos(target), self.ent_bbox(target)) else { return };
        let hand = if m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:bow") { 0 } else { 1 };
        let bow = m.equipment[hand].clone();
        let ammo = held_arrow(m).unwrap_or_else(|| Stack { id: "minecraft:arrow".into(), count: 1, components: None });
        let mut a = self.mob_create_arrow(m, &ammo, bow.as_ref());
        // On the arrow's random.
        let diff = self.level.difficulty_id();
        a.damage = f64::from(power * 2.0) + a.rng.triangle(f64::from(diff) * 0.11, 0.57425);
        // F01: variants (effect added to the stack, colour updated).
        if let Some((eff, dur)) = super::variants::arrow_effect(m.kind)
            && a.kind == PKind::Arrow
        {
            add_arrow_effect(&mut a, eff, dur);
        }
        // F12: wither skeleton, burning arrow 100 s.
        if super::nether::flaming_arrows(m.kind) {
            a.fire = a.fire.max(2000);
            a.sync_fire_flag();
        }
        let xd = tp[0] - m.pos[0];
        let yd = tb.min[1] + (tb.max[1] - tb.min[1]) * 0.333_333_333_333_333_3 - a.pos[1];
        let zd = tp[2] - m.pos[2];
        let dist = (xd * xd + zd * zd).sqrt();
        let unc = (14 - diff * 4) as f32;
        a.shoot([xd, yd + dist * f64::from(0.2f32), zd], 1.6, unc);
        self.proj_spawn(a, &ammo);
        let p = 1.0 / (m.rng.next_float() * 0.4 + 0.8);
        self.mob_sound(m, "minecraft:entity.skeleton.shoot", 1.0, p);
    }

    /// Wear of the item held by a mob (Unbreaking on the world's random);
    /// broken: stack removed, break event (47 main hand, 48 off hand).
    pub(crate) fn mob_hurt_item(&mut self, m: &mut Mob, hand: usize, n: i32) {
        let Some(s) = m.equipment[hand].clone() else { return };
        let items = &self.life.items;
        if !items.damageable(&s) {
            return;
        }
        let unbreaking = items.enchantment(&s, "minecraft:unbreaking");
        let mut n2 = n;
        if unbreaking > 0 {
            let p = unbreaking as f32 / (unbreaking as f32 + 1.0);
            for _ in 0..n {
                if self.life.rng.next_float() < p {
                    n2 -= 1;
                }
            }
        }
        if n2 <= 0 {
            return;
        }
        let max = self.life.items.max_damage(&s);
        let dmg = self.life.items.damage(&s) + n2;
        let mut s = s;
        if dmg >= max {
            s.count -= 1;
            crate::items::remove_component(&mut s, "minecraft:damage");
            m.equipment[hand] = (s.count > 0).then_some(s);
            let event = if hand == 0 { 47 } else { 48 };
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Event { id: m.id, event }));
        } else {
            crate::items::set_component(&mut s, "minecraft:damage", Nbt::Int(dmg));
            m.equipment[hand] = Some(s);
        }
        self.send_mob_equipment(m, &[hand]);
    }

    /// Impact point of the segment on the collision shapes (fraction from `from` to `to`), with
    /// the game's cell walk (bounds extended by 1e-7) and its box test (0 < s < 1,
    /// tolerance 1e-7 on the faces, interior start point).
    pub(crate) fn clip_t(&self, from: [f64; 3], to: [f64; 3]) -> Option<f64> {
        self.clip_hit(from, to).map(|h| h.0)
    }

    /// Like `clip_t`, with the cell touched and the normal of the face touched.
    pub(crate) fn clip_hit(&self, from: [f64; 3], to: [f64; 3]) -> Option<(f64, [i32; 3], [i32; 3])> {
        if from == to {
            return None;
        }
        let lerp = |k: f64, a: f64, b: f64| a + k * (b - a);
        let te: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, to[i], from[i]));
        let fe: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, from[i], to[i]));
        let mut cell = fe.map(|v| v.floor() as i32);
        if let Some((t, n)) = self.clip_cell(cell, from, to) {
            return Some((t, cell, n));
        }
        let d: [f64; 3] = std::array::from_fn(|i| te[i] - fe[i]);
        let sign = d.map(|v| {
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        });
        let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / d[i] });
        let frac = |v: f64| v - v.floor();
        let mut t: [f64; 3] =
            std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(fe[i]) } else { frac(fe[i]) });
        while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
            let a = if t[0] < t[1] {
                if t[0] < t[2] { 0 } else { 2 }
            } else if t[1] < t[2] {
                1
            } else {
                2
            };
            cell[a] += sign[a];
            t[a] += delta[a];
            if let Some((s, n)) = self.clip_cell(cell, from, to) {
                return Some((s, cell, n));
            }
        }
        None
    }

    /// Test of a cell on the original segment.
    fn clip_cell(&self, cell: [i32; 3], from: [f64; 3], to: [f64; 3]) -> Option<(f64, [i32; 3])> {
        let mut boxes = Vec::new();
        self.phys.collision(self.st(cell), &mut boxes);
        if boxes.is_empty() {
            return None;
        }
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 1.0e-7 {
            return None;
        }
        let p = [from[0] + d[0] * 0.001, from[1] + d[1] * 0.001, from[2] + d[2] * 0.001];
        let c = cell.map(f64::from);
        if boxes.iter().any(|b| (0..3).all(|k| p[k] - c[k] >= b.min[k] && p[k] - c[k] < b.max[k])) {
            // Interior origin: face opposite to the direction closest to the movement
            // (order down, up, north, south, west, east; the first of the maximum).
            let cand = [(1usize, -d[1], -1), (1, d[1], 1), (2, -d[2], -1), (2, d[2], 1), (0, -d[0], -1), (0, d[0], 1)];
            let mut top = cand[0];
            for c in &cand[1..] {
                if c.1 > top.1 {
                    top = *c;
                }
            }
            let mut n = [0; 3];
            n[top.0] = -top.2;
            return Some((0.001, n));
        }
        let mut best = 1.0f64;
        let mut normal = [0; 3];
        let mut hit = false;
        for b in &boxes {
            let bb = b.offset(c[0], c[1], c[2]);
            for a in 0..3 {
                let (b1, b2) = ((a + 1) % 3, (a + 2) % 3);
                let plane = if d[a] > 1.0e-7 {
                    bb.min[a]
                } else if d[a] < -1.0e-7 {
                    bb.max[a]
                } else {
                    continue;
                };
                let s = (plane - from[a]) / d[a];
                let pb = from[b1] + s * d[b1];
                let pc = from[b2] + s * d[b2];
                if 0.0 < s
                    && s < best
                    && bb.min[b1] - 1.0e-7 < pb
                    && pb < bb.max[b1] + 1.0e-7
                    && bb.min[b2] - 1.0e-7 < pc
                    && pc < bb.max[b2] + 1.0e-7
                {
                    best = s;
                    hit = true;
                    normal = [0; 3];
                    normal[a] = if d[a] > 0.0 { -1 } else { 1 };
                }
            }
        }
        hit.then_some((best, normal))
    }
}

/// Effect added to the carried stack's own effects, colour (data 11).
pub(crate) fn add_arrow_effect(a: &mut Proj, id: &str, dur: i32) {
    let e = crate::effects::Inst::new(id, dur, 0).to_nbt();
    let mut c =
        crate::items::component(&a.item, "minecraft:potion_contents").cloned().unwrap_or(Nbt::Compound(Vec::new()));
    if let Nbt::String(potion) = &c {
        c = Nbt::Compound(vec![("potion".into(), Nbt::String(potion.clone()))]);
    }
    let mut list: Vec<Nbt> = crate::nbtio::list(c.get("custom_effects")).to_vec();
    list.push(e);
    crate::nbtio::set(&mut c, "custom_effects", Nbt::List(list));
    crate::items::set_component(&mut a.item, "minecraft:potion_contents", c);
    let color = crate::projectiles::hits::potion_color(&a.item);
    a.set_meta(11, Meta::Int(color));
}

/// Entry of a segment (origin `o`, displacement `d`) into a box, fraction in [0, 1].
pub fn ray_t(b: &rc_physics::Aabb, o: [f64; 3], d: [f64; 3]) -> Option<f64> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for a in 0..3 {
        if d[a].abs() < 1.0e-12 {
            if o[a] < b.min[a] || o[a] > b.max[a] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[a];
        let (mut ta, mut tb) = ((b.min[a] - o[a]) * inv, (b.max[a] - o[a]) * inv);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}
