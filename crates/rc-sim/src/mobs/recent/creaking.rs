//! Creaking (`server-mobs.md` § 4.F14): brain (core, idle, fight), frozen when a
//! player looks at it (then activates within 12 blocks), linked to its creaking heart
//! (invulnerable as long as the heart protects it, tears down in 45 ticks when the heart is
//! broken), path limited to 32 blocks from the heart.

use super::super::brainy::brain::{Act, Bk, Brain, K, Sn, Spd, St, act, core, look_sometimes, move_sink, run_one, seq};
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, cos, sin};
use super::xbrain::{Find, Hook, Stop, XBk};
use crate::server::Server;

/// Activation distance (squared): 12 blocks.
const ACTIVATION_RANGE_SQ: f64 = 144.0;

pub fn brain(rng: &mut JRand) -> Brain {
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::X(XBk::Wrap { inner: Box::new(Bk::Swim { chance: 0.8 }), hook: Hook::CanMove }),
            Bk::LookSink { lo: 45, hi: 90 },
            move_sink(),
        ],
    );
    let idle = seq(
        10,
        Act::Idle,
        vec![
            Bk::X(XBk::StartAttack(Find::CreakingPlayer)),
            look_sometimes(8.0, false),
            run_one(
                &[],
                0x4352_4541,
                vec![
                    (Bk::Stroll { speed: 0.3, from_water: true }, 2),
                    (Bk::WalkFromLook { speed: Spd::Fixed(0.3), close: 3, axolotl: false }, 2),
                    (Bk::DoNothing { lo: 30, hi: 60 }, 1),
                ],
            ),
        ],
    );
    let fight = seq(
        10,
        Act::Fight,
        vec![
            Bk::WalkToAttack(Spd::Fixed(1.0)),
            Bk::X(XBk::Wrap { inner: Box::new(Bk::Melee { cd: 40 }), hook: Hook::CanMove }),
            Bk::X(XBk::StopAttack { why: Stop::CreakingUnreachable, positives: 0 }),
        ],
    );
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Fight, &[(K::Attack, St::Present)], &[]), fight),
        ],
    )
}

/// Frozen → idle; otherwise fight, then idle.
pub fn update_activity(m: &Mob, b: &mut Brain) {
    if !m.rec.can_move {
        b.use_default();
    } else {
        b.first_valid(&[Act::Fight, Act::Idle]);
    }
}

/// Linked heart (own path maluses follow `home`).
pub fn set_transient(m: &mut Mob, home: [i32; 3]) {
    m.rec.home = Some(home);
}

/// A creaking's tick (server): without its heart's protection, the linked creaking dies.
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    let Some(home) = m.rec.home else { return };
    if !s.heart_is_protector(home, m) {
        m.health = 0.0;
    }
}

/// Linked creaking that tears down (45 ticks, then breaks apart).
pub fn tick_death(s: &mut Server, m: &mut Mob) -> bool {
    if m.rec.home.is_none() || !m.rec.tearing_down {
        return false;
    }
    m.death_time += 1;
    if m.death_time > 45 && !m.removed {
        tear_down(s, m);
    }
    true
}

/// Pale oak wood particles (100) and awake heart particles (10), death sound, removal.
pub fn tear_down(s: &mut Server, m: &mut Mob) {
    let b = m.bbox();
    let c = [(b.min[0] + b.max[0]) * 0.5, (b.min[1] + b.max[1]) * 0.5, (b.min[2] + b.max[2]) * 0.5];
    let spread = [
        ((b.max[0] - b.min[0]) * 0.3) as f32,
        ((b.max[1] - b.min[1]) * 0.3) as f32,
        ((b.max[2] - b.min[2]) * 0.3) as f32,
    ];
    if let Some(wood) = s.default_state_of("minecraft:pale_oak_wood") {
        s.state_particles("minecraft:block_crumble", wood, c, spread, 0.0, 100);
    }
    if let Some(heart) = s.default_state_of("minecraft:creaking_heart")
        && let Some(awake) = s.with_prop(heart, "creaking_heart_state", "awake")
    {
        s.state_particles("minecraft:block_crumble", awake, c, spread, 0.0, 10);
    }
    let death = m.death_sound();
    s.make_sound(m, death);
    m.removed = true;
}

/// Death attributed to the source, convulsion sound; then tears down.
pub fn death_effects(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) {
    if by == Some(Ent::Player) {
        m.last_hurt_by_player = 100;
    }
    s.mob_die(m, ty, by);
    s.make_sound(m, "minecraft:entity.creaking.twitch");
    m.rec.tearing_down = true;
    m.health = 0.0;
}

/// The overridden server-side damage computation: linked creaking, invulnerable; the heart suffers if a player hits.
pub fn before_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) -> Option<bool> {
    let home = m.rec.home?;
    if s.dmg_tag(ty, "minecraft:bypasses_invulnerability") {
        return None;
    }
    if m.rec.invul_anim > 0 || m.dead || m.health <= 0.0 {
        return Some(false);
    }
    // Living direct source or projectile, or responsible player.
    let player = by == Some(Ent::Player);
    let direct_ok = by.is_some() || s.dmg_tag(ty, "minecraft:is_projectile");
    if !direct_ok && !player {
        return Some(false);
    }
    if player {
        m.last_hurt_by_player = 100;
    }
    m.rec.invul_anim = 8;
    s.mob_event(m, 66);
    s.mob_game_event(m, "minecraft:entity_action");
    if s.heart_is_protector(home, m) {
        if player {
            s.heart_creaking_hurt(home, m);
        }
        let h = super::hurt_sound(m, false).unwrap_or_else(|| m.hurt_sound());
        s.make_sound(m, h);
    }
    Some(true)
}

/// The AI step (start): animations, then "can move" recomputed (freeze and unfreeze sounds).
pub fn pre_ai_step(s: &mut Server, m: &mut Mob) {
    if m.rec.invul_anim > 0 {
        m.rec.invul_anim -= 1;
    }
    if m.rec.attack_anim > 0 {
        m.rec.attack_anim -= 1;
    }
    let was = m.rec.can_move;
    let now = check_can_move(s, m);
    if now != was {
        s.mob_game_event(m, "minecraft:entity_action");
        if now {
            s.make_sound(m, "minecraft:entity.creaking.unfreeze");
        } else {
            // Navigation cut, no movement input.
            m.nav.stop();
            m.xxa = 0.0;
            m.yya = 0.0;
            m.zza = 0.0;
            m.speed = 0.0;
            m.vel = [0.0; 3];
            s.make_sound(m, "minecraft:entity.creaking.freeze");
        }
    }
    m.rec.can_move = now;
}

/// Frozen if an attackable player looks at it; activates within 12 blocks.
fn check_can_move(s: &mut Server, m: &mut Mob) -> bool {
    let players: Vec<Ent> = m.bm.brain.as_ref().and_then(|b| b.mem.players.v.clone()).unwrap_or_default();
    let active = m.rec.active;
    if players.is_empty() {
        if active {
            deactivate(s, m);
        }
        return true;
    }
    let mut potential = false;
    for p in players {
        if p != Ent::Player || !s.bz_mob_can_attack(m, p) {
            continue;
        }
        potential = true;
        let disguised = s.slots[5].as_ref().is_some_and(|h| s.item_tag(&h.id, "minecraft:gaze_disguise_equipment"));
        let eye = m.eye_y();
        let heights = [eye, m.pos[1] + 0.5, (eye + m.pos[1]) / 2.0];
        if (!active || !disguised) && s.player_looks_at(m, 0.5, &heights) {
            if active {
                return false;
            }
            let pp = s.pos;
            let d = (0..3).map(|i| (pp[i] - m.pos[i]).powi(2)).sum::<f64>();
            if d < ACTIVATION_RANGE_SQ {
                activate(s, m);
                return false;
            }
        }
    }
    if !potential && active {
        deactivate(s, m);
    }
    true
}

fn activate(s: &mut Server, m: &mut Mob) {
    if let Some(b) = m.bm.brain.as_mut() {
        b.mem.attack.set(Ent::Player);
    }
    s.mob_game_event(m, "minecraft:entity_action");
    s.make_sound(m, "minecraft:entity.creaking.activate");
    m.rec.active = true;
}

fn deactivate(s: &mut Server, m: &mut Mob) {
    if let Some(b) = m.bm.brain.as_mut() {
        b.mem.attack.erase();
    }
    s.mob_game_event(m, "minecraft:entity_action");
    s.make_sound(m, "minecraft:entity.creaking.deactivate");
    m.rec.active = false;
}

/// The player's eyes in the box more than 4 tests in a row.
pub fn player_stuck_in_you(s: &Server, m: &mut Mob) -> bool {
    let players = m.bm.brain.as_ref().and_then(|b| b.mem.players.v.clone()).unwrap_or_default();
    if players.is_empty() {
        m.rec.stuck_counter = 0;
        return false;
    }
    let b = m.bbox();
    let eye = [s.pos[0], s.player_eye_y(), s.pos[2]];
    if players.contains(&Ent::Player) && (0..3).all(|i| eye[i] >= b.min[i] && eye[i] < b.max[i]) {
        m.rec.stuck_counter += 1;
        return m.rec.stuck_counter > 4;
    }
    m.rec.stuck_counter = 0;
    false
}

impl Server {
    /// The player's gaze passes
    /// through the cone toward one of the heights, and the view (visual shapes) is clear.
    pub(crate) fn player_looks_at(&self, m: &Mob, cone: f64, heights: &[f64]) -> bool {
        let (f, g) = (self.pitch * DEG, -self.yaw * DEG);
        let (h, i, j, k) = (cos(g), sin(g), cos(f), sin(f));
        let look = [f64::from(i * j), f64::from(-k), f64::from(h * j)];
        let ll = (look[0] * look[0] + look[1] * look[1] + look[2] * look[2]).sqrt();
        let look = if ll < 1.0e-5 { [0.0; 3] } else { look.map(|v| v / ll) };
        let eye = self.player_eye_y();
        for &gh in heights {
            let dir = [m.pos[0] - self.pos[0], gh - eye, m.pos[2] - self.pos[2]];
            let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            let n = if l < 1.0e-5 { [0.0; 3] } else { dir.map(|v| v / l) };
            let dot = look[0] * n[0] + look[1] * n[1] + look[2] * n[2];
            if dot > 1.0 - cone {
                let from = [self.pos[0], eye, self.pos[2]];
                let to = [m.pos[0], gh, m.pos[2]];
                let d = ((0..3).map(|x| (to[x] - from[x]).powi(2)).sum::<f64>()).sqrt();
                if d <= 128.0 && !self.clip_visual(from, to) {
                    return true;
                }
            }
        }
        false
    }

    /// Does the segment touch a block's visual shape? Glass,
    /// panes, bars and powder snow hide nothing; fences at outline height;
    /// full mud and soul sand; layered snow according to its thickness.
    pub(crate) fn clip_visual(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        if from == to {
            return false;
        }
        let lerp = |k: f64, a: f64, b: f64| a + k * (b - a);
        let te: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, to[i], from[i]));
        let fe: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, from[i], to[i]));
        let mut cell = fe.map(|v| v.floor() as i32);
        if self.visual_cell_hit(cell, from, to) {
            return true;
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
            if self.visual_cell_hit(cell, from, to) {
                return true;
            }
        }
        false
    }

    pub(crate) fn visual_cell_hit(&self, cell: [i32; 3], from: [f64; 3], to: [f64; 3]) -> bool {
        let boxes = self.visual_boxes(cell);
        if boxes.is_empty() {
            return false;
        }
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let c = cell.map(f64::from);
        let p = [from[0] + d[0] * 0.001, from[1] + d[1] * 0.001, from[2] + d[2] * 0.001];
        if boxes.iter().any(|b| (0..3).all(|k| p[k] - c[k] >= b.min[k] && p[k] - c[k] < b.max[k])) {
            return true;
        }
        boxes.iter().any(|b| super::super::arrows::ray_t(&b.offset(c[0], c[1], c[2]), from, d).is_some())
    }

    /// Visual shapes of a cell (block units).
    pub(crate) fn visual_boxes(&self, p: [i32; 3]) -> Vec<rc_physics::Aabb> {
        let s = self.st(p);
        let name = self.state_name(s).trim_start_matches("minecraft:");
        let unit = |h: f64| rc_physics::Aabb { min: [0.0; 3], max: [1.0, h, 1.0] };
        let transparent = name == "glass"
            || name == "tinted_glass"
            || name.ends_with("_stained_glass")
            || name.ends_with("_pane")
            || name == "iron_bars"
            || name == "powder_snow"
            || (name.contains("copper_grate"));
        if transparent {
            return Vec::new();
        }
        if name == "mud" || name == "soul_sand" {
            return vec![unit(1.0)];
        }
        if name == "snow" {
            let layers =
                self.data.state_properties(s).find(|(k, _)| *k == "layers").and_then(|(_, v)| v.parse::<f64>().ok());
            return vec![unit(layers.unwrap_or(1.0) * 2.0 / 16.0)];
        }
        let mut boxes = Vec::new();
        self.phys.collision(s, &mut boxes);
        if name.ends_with("_fence") {
            for b in boxes.iter_mut() {
                b.max[1] = b.max[1].min(1.0);
            }
        }
        boxes
    }
}
