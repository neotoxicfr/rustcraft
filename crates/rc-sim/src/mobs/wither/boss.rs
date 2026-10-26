//! Wither behavior (`server-mobs.md` § 4.F16): push toward the main target,
//! side heads (random choice, pending "blue" shots), summoning (220 ticks, healing,
//! explosion 7), destruction of the blocks around after a hit, slow healing, purple boss bar,
//! damage rules, Nether star, wither rose from victims.

use rc_client::Event;
use rc_proto_v777::play::{BossEvent, BossOp};

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth;
use super::super::species::Kind;
use super::{INVULNERABLE_TICKS, display_name, powered, silent};
use crate::player::Stack;
use crate::server::Server;

/// Purple color, continuous bar, darkened screen.
const BAR_COLOR: i32 = 5;
const BAR_STYLE: i32 = 0;
const BAR_FLAGS: u8 = 1;

/// Living entity with network id `id`.
pub(crate) fn ent_of(s: &Server, id: i32) -> Option<Ent> {
    if id <= 0 {
        None
    } else if id == s.player_id {
        Some(Ent::Player)
    } else {
        s.mob_ref(id).map(|_| Ent::Mob(id))
    }
}

/// Network id of a living entity.
fn net_id(s: &Server, e: Ent) -> i32 {
    match e {
        Ent::Player => s.player_id,
        Ent::Mob(id) => id,
    }
}

/// Side heads at 1.3 blocks on either side of the body (index 3 ≡ index 1).
pub fn head_x(m: &Mob, i: i32) -> f64 {
    if i <= 0 {
        return m.pos[0];
    }
    let a = (m.body_yaw + (180 * (i - 1)) as f32) * mth::DEG;
    m.pos[0] + f64::from(mth::cos(a)) * 1.3
}

pub fn head_y(m: &Mob, i: i32) -> f64 {
    m.pos[1] + f64::from(if i <= 0 { 3.0f32 } else { 2.2f32 })
}

pub fn head_z(m: &Mob, i: i32) -> f64 {
    if i <= 0 {
        return m.pos[2];
    }
    let a = (m.body_yaw + (180 * (i - 1)) as f32) * mth::DEG;
    m.pos[2] + f64::from(mth::sin(a)) * 1.3
}

/// A float draw between two bounds.
fn next_double(r: &mut JRand, min: f64, max: f64) -> f64 {
    if min >= max { min } else { r.next_double() * (max - min) + min }
}

fn heal(m: &mut Mob, amount: f32) {
    if m.health > 0.0 {
        m.health = (m.health + amount).clamp(0.0, m.max_health());
    }
}

/// Start of the AI step: damped vertical velocity, rise and push toward the center head's
/// target, body turned toward the movement.
pub fn pre_ai_step(s: &Server, m: &mut Mob) {
    if m.kind != Kind::Wither {
        return;
    }
    let mut v = [m.vel[0], m.vel[1] * 0.6, m.vel[2]];
    if let Some(p) = ent_of(s, m.wither.targets[0]).and_then(|e| s.ent_pos(e)) {
        let mut yd = v[1];
        if m.pos[1] < p[1] || (!powered(m) && m.pos[1] < p[1] + 5.0) {
            yd = yd.max(0.0);
            yd += 0.3 - yd * f64::from(0.6f32);
        }
        v[1] = yd;
        let (dx, dz) = (p[0] - m.pos[0], p[2] - m.pos[2]);
        if dx * dx + dz * dz > 9.0 {
            let l = (dx * dx + dz * dz).sqrt();
            let (nx, nz) = if l < 1.0e-5 { (0.0, 0.0) } else { (dx / l, dz / l) };
            v[0] += nx * 0.3 - v[0] * 0.6;
            v[2] += nz * 0.3 - v[2] * 0.6;
        }
    }
    m.vel = v;
    if v[0] * v[0] + v[2] * v[2] > 0.05 {
        m.yaw = mth::atan2(v[2], v[0]) as f32 * (180.0f32 / std::f32::consts::PI) - 90.0;
    }
}

/// End of the AI step: rotations of the side heads, then particle draws (smoke, armor
/// effect, summoning): no effect server-side but the random source advances.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    if m.kind != Kind::Wither {
        return;
    }
    for i in 0..2 {
        let target = ent_of(s, m.wither.targets[i + 1]);
        match target.and_then(|e| Some((s.ent_pos(e)?, s.ent_eye(e)?))) {
            Some((p, eye)) => {
                let h = i as i32 + 1;
                let (xd, yd, zd) = (p[0] - head_x(m, h), eye - head_y(m, h), p[2] - head_z(m, h));
                let sd = (xd * xd + zd * zd).sqrt();
                let pi = f64::from(std::f32::consts::PI);
                let y_rot = (mth::atan2(zd, xd) * 180.0 / pi) as f32 - 90.0;
                let x_rot = (-(mth::atan2(yd, sd) * 180.0 / pi)) as f32;
                m.wither.x_rot_heads[i] = mth::rotlerp(m.wither.x_rot_heads[i], x_rot, 40.0);
                m.wither.y_rot_heads[i] = mth::rotlerp(m.wither.y_rot_heads[i], y_rot, 10.0);
            }
            None => m.wither.y_rot_heads[i] = mth::rotlerp(m.wither.y_rot_heads[i], m.body_yaw, 10.0),
        }
    }
    let armored = powered(m);
    for _ in 0..3 {
        for _ in 0..3 {
            m.rng.next_gaussian();
        }
        if armored && s.mobs.rng.next_int_bound(4) == 0 {
            for _ in 0..3 {
                m.rng.next_gaussian();
            }
        }
    }
    if m.wither.inv > 0 {
        for _ in 0..3 {
            m.rng.next_gaussian();
            m.rng.next_float();
            m.rng.next_gaussian();
        }
    }
}

/// Summoning, side heads, center target, destroyed blocks, healing,
/// boss bar.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    if m.kind != Kind::Wither {
        return;
    }
    s.mobs.wither.ticking = Some(m.id);
    if m.wither.inv > 0 {
        let n = m.wither.inv - 1;
        set_progress(s, m, 1.0 - n as f32 / INVULNERABLE_TICKS as f32);
        if n <= 0 {
            let c = [m.pos[0], m.eye_y(), m.pos[2]];
            s.explode_core(Some(m), Some(Ent::Mob(m.id)), Some(m.id), c, 7.0, false, "minecraft:player_explosion");
            if !silent(m) {
                s.global_level_event(1023, m.block_pos(), 0);
            }
        }
        m.wither.inv = n;
        if m.tick_count % 10 == 0 {
            heal(m, 10.0);
        }
    } else {
        side_heads(s, m);
        m.wither.targets[0] = m.target.map_or(0, |t| net_id(s, t));
        destroy_blocks(s, m);
        if m.tick_count % 20 == 0 {
            heal(m, 1.0);
        }
        let p = m.health / m.max_health();
        set_progress(s, m, p);
    }
    s.mobs.wither.ticking = None;
}

/// Heads 1 and 2: next update, random "blue" shot after 15 waits (normal and
/// hard), shot at their target or choice of a new target.
fn side_heads(s: &mut Server, m: &mut Mob) {
    for i in 1..3usize {
        if m.tick_count < m.wither.next_head_update[i - 1] {
            continue;
        }
        m.wither.next_head_update[i - 1] = m.tick_count + 10 + m.rng.next_int_bound(10);
        let d = s.level.difficulty_id();
        if d == 2 || d == 3 {
            let idle = m.wither.idle_head_updates[i - 1];
            m.wither.idle_head_updates[i - 1] += 1;
            if idle > 15 {
                let xt = next_double(&mut m.rng, m.pos[0] - 10.0, m.pos[0] + 10.0);
                let yt = next_double(&mut m.rng, m.pos[1] - 5.0, m.pos[1] + 5.0);
                let zt = next_double(&mut m.rng, m.pos[2] - 10.0, m.pos[2] + 10.0);
                // Quirk of the game: the side heads shoot from indices 2 and 3
                // (index 3 falls back onto head 1's position).
                shoot(s, m, i as i32 + 1, [xt, yt, zt], true);
                m.wither.idle_head_updates[i - 1] = 0;
            }
        }
        let head_target = m.wither.targets[i];
        if head_target > 0 {
            let cur = ent_of(s, head_target);
            let ok = cur.is_some_and(|c| {
                s.targeting_ok(m, c, true, 0.0, false, false)
                    && s.ent_pos(c).is_some_and(|p| mth::dist2(m.pos, p) <= 900.0)
                    && s.line_of_sight(m, c)
            });
            match cur.filter(|_| ok) {
                Some(c) => {
                    shoot_at(s, m, i as i32 + 1, c);
                    m.wither.next_head_update[i - 1] = m.tick_count + 40 + m.rng.next_int_bound(20);
                    m.wither.idle_head_updates[i - 1] = 0;
                }
                None => m.wither.targets[i] = 0,
            }
        } else {
            let list = nearby_targets(s, m);
            if !list.is_empty() {
                let k = m.rng.next_int_bound(list.len() as i32) as usize;
                m.wither.targets[i] = net_id(s, list[k]);
            }
        }
    }
}

/// Living entities targetable by a head (box inflated by 20 / 8 / 20, range 20, sight,
/// excluding undead): mobs in list order, then the player.
fn nearby_targets(s: &Server, m: &mut Mob) -> Vec<Ent> {
    let bb = m.bbox();
    let area = rc_physics::Aabb {
        min: [bb.min[0] - 20.0, bb.min[1] - 8.0, bb.min[2] - 20.0],
        max: [bb.max[0] + 20.0, bb.max[1] + 8.0, bb.max[2] + 20.0],
    };
    let mut cands: Vec<Ent> = s
        .mobs
        .list
        .iter()
        .flatten()
        .filter(|o| o.id != m.id && !o.kind.undead() && o.touches(&area))
        .map(|o| Ent::Mob(o.id))
        .collect();
    if s.player_aabb().intersects(&area) {
        cands.push(Ent::Player);
    }
    cands.retain(|e| s.targeting_ok(m, *e, true, 20.0, true, true));
    cands
}

/// Toward the middle of the eye height; the center
/// head shoots a blue skull one time in a thousand.
pub(crate) fn shoot_at(s: &mut Server, m: &mut Mob, head: i32, t: Ent) {
    let (Some(p), Some(eye)) = (s.ent_pos(t), s.ent_eye(t)) else { return };
    let ty = p[1] + (eye - p[1]) * 0.5;
    let dangerous = head == 0 && m.rng.next_float() < 0.001;
    shoot(s, m, head, [p[0], ty, p[2]], dangerous);
}

/// Shot of a skull from a head (event 1024, skull at the head, normalized direction).
pub(crate) fn shoot(s: &mut Server, m: &Mob, head: i32, t: [f64; 3], dangerous: bool) {
    if !silent(m) {
        s.level_event(1024, m.block_pos(), 0);
    }
    let h = [head_x(m, head), head_y(m, head), head_z(m, head)];
    let d = [t[0] - h[0], t[1] - h[1], t[2] - h[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let dir = if l < 1.0e-5 { [0.0; 3] } else { d.map(|v| v / l) };
    s.shoot_wither_skull(m.uuid, h, dir, (m.yaw, m.pitch), dangerous);
}

/// Blocks around destroyed 20 ticks after a hit (`mob_griefing`), event 1022 if one
/// of them fell.
fn destroy_blocks(s: &mut Server, m: &mut Mob) {
    if m.wither.destroy_blocks_tick <= 0 {
        return;
    }
    m.wither.destroy_blocks_tick -= 1;
    if m.wither.destroy_blocks_tick != 0 || !s.rule("minecraft:mob_griefing", true) {
        return;
    }
    let w = (m.width() / 2.0 + 1.0).floor() as i32;
    let h = m.height().floor() as i32;
    let b = m.block_pos();
    let mut destroyed = false;
    // X, then y, then z.
    for z in b[2] - w..=b[2] + w {
        for y in b[1]..=b[1] + h {
            for x in b[0] - w..=b[0] + w {
                let p = [x, y, z];
                if can_destroy(s, p) {
                    destroyed = s.mob_destroy_block(p, true, m.id) || destroyed;
                }
            }
        }
    }
    if destroyed {
        s.level_event(1022, b, 0);
    }
}

/// Neither air nor a block of the `wither_immune` tag.
pub fn can_destroy(s: &Server, p: [i32; 3]) -> bool {
    !s.is_air(p) && !s.btag_id(p, crate::tag_bits::TagId::WitherImmune)
}

// ---------- Damage ----------

/// Is the wither (or one of its undead friends) the author?
fn by_friend(s: &Server, by: Option<Ent>) -> bool {
    // During its own melee the attacker is out of the list: its type is in `killer`.
    matches!(by, Some(Ent::Mob(id)) if s.mobs.wither.ticking == Some(id)
        || s.mob_ref(id).is_some_and(|o| o.kind.undead())
        || s.mobs.killer.as_ref().is_some_and(|k| k.2 == id && k.1.undead()))
}

/// The wither's server-side damage computation before the common damage; `Some(false)`: hit refused.
pub fn before_hurt(s: &Server, m: &mut Mob, ty: &str, by: Option<Ent>) -> Option<bool> {
    if m.kind != Kind::Wither {
        return None;
    }
    // Immune to fire (no effect on the state).
    if s.dmg_tag(ty, "minecraft:is_fire") {
        return Some(false);
    }
    if s.dmg_tag(ty, "minecraft:wither_immune_to") {
        return Some(false);
    }
    if m.wither.inv > 0 && !s.dmg_tag(ty, "minecraft:bypasses_invulnerability") {
        return Some(false);
    }
    // Armor: arrows, tridents and wind charges have no effect (direct source).
    if powered(m) && matches!(ty, "minecraft:arrow" | "minecraft:trident" | "minecraft:wind_charge") {
        return Some(false);
    }
    // Wither or undead author (`wither_friends`).
    if by_friend(s, by) {
        return Some(false);
    }
    if m.wither.destroy_blocks_tick <= 0 {
        m.wither.destroy_blocks_tick = 20;
    }
    for v in &mut m.wither.idle_head_updates {
        *v += 3;
    }
    None
}

// ---------- Boss bar ----------

fn send_bar(s: &Server, m: &Mob, op: BossOp) {
    let _ = s.send(Event::Boss(BossEvent { id: m.wither.bar, op }));
}

/// Sent to the registered players if it changes.
fn set_progress(s: &Server, m: &mut Mob, p: f32) {
    if p != m.wither.bar_progress {
        m.wither.bar_progress = p;
        if m.wither.bar_shown {
            send_bar(s, m, BossOp::Progress(p));
        }
    }
}

/// After network tracking: player registered when they see the wither,
/// removed otherwise; custom name changed.
pub fn bar_track(s: &Server, m: &mut Mob) {
    if m.kind != Kind::Wither {
        return;
    }
    if m.custom_name != m.wither.bar_custom {
        m.wither.bar_custom = m.custom_name.clone();
        m.wither.bar_name = display_name(m);
        if m.wither.bar_shown {
            send_bar(s, m, BossOp::Name(Some(m.wither.bar_name.clone())));
        }
    }
    let want = m.track.seen && !m.removed;
    if want && !m.wither.bar_shown {
        m.wither.bar_shown = true;
        let op = BossOp::Add {
            name: Some(m.wither.bar_name.clone()),
            progress: m.wither.bar_progress,
            color: BAR_COLOR,
            style: BAR_STYLE,
            flags: BAR_FLAGS,
        };
        send_bar(s, m, op);
    } else if !want && m.wither.bar_shown {
        m.wither.bar_shown = false;
        send_bar(s, m, BossOp::Remove);
    }
}

/// Wither removed from the world: the player leaves the bar.
pub fn bar_removed(s: &Server, bar: u128) {
    let _ = s.send(Event::Boss(BossEvent { id: bar, op: BossOp::Remove }));
}

// ---------- Death ----------

impl Server {
    /// Is the credited killer a wither?
    fn wither_credit(&self, by: Option<(Ent, i32)>, by_player: bool) -> bool {
        !by_player
            && matches!(by, Some((Ent::Mob(id), _))
                if self.mobs.wither.ticking == Some(id) || self.mob_ref(id).is_some_and(|o| o.kind == Kind::Wither))
    }

    /// The wither's own death loot: Nether star with a long lifetime.
    pub(crate) fn wither_death_loot(&mut self, m: &Mob) {
        if m.kind != Kind::Wither {
            return;
        }
        let r = &mut self.mobs.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        let star = Stack { id: "minecraft:nether_star".into(), count: 1, components: None };
        let before = self.life.drops.len();
        self.spawn_item(m.pos, v, star, 10);
        if self.life.drops.len() > before
            && let Some(d) = self.life.drops.last_mut()
        {
            // The lifetime extension.
            d.age = -6000;
        }
    }

    /// Creation of a wither rose from a mob killed by a wither: rose placed in place (`mob_griefing`
    /// rule, empty cell, support), otherwise dropped.
    pub(crate) fn wither_rose_mob(&mut self, m: &Mob) {
        if self.wither_credit(m.last_hurt_by_mob, m.last_hurt_by_player > 0) {
            self.wither_rose_at(m.pos);
        }
    }

    /// Same rule on the player's death.
    pub(crate) fn wither_rose_player(&mut self) {
        if self.wither_credit(self.mobs.player_hurt_by, false) {
            let p = self.pos;
            self.wither_rose_at(p);
        }
    }

    fn wither_rose_at(&mut self, pos: [f64; 3]) {
        let bp = mth::block_of(pos);
        let mut placed = false;
        if self.rule("minecraft:mob_griefing", true)
            && let Some(rose) = self.default_state_of("minecraft:wither_rose")
            && self.is_air(bp)
            && self.can_survive(rose, bp)
        {
            self.set_block(bp, rose, crate::blocks::flag::ALL);
            placed = true;
        }
        if !placed {
            let r = &mut self.mobs.rng;
            let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
            let rose = Stack { id: "minecraft:wither_rose".into(), count: 1, components: None };
            self.spawn_item(pos, v, rose, 0);
        }
    }
}
