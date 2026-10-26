//! Mob status effects (family F01, hooked onto the player effects of S7,
//! `effects.rs`): same instances (`Inst`: hidden effect, infinite duration), same effect
//! table (colours, attribute modifiers), rules of game 26.3 for mobs:
//! acceptance, tick (regeneration, poison, wither,
//! instant effects inverted for the undead), modifiers (speed, damage, max health),
//! particles and flags (invisible, glowing) in the entity data. The game does not
//! send a mob's effects through `update_mob_effect` (reserved for the player and their
//! mount): only the entity data shows them.

use super::mob::{Ent, Mob};
use super::species::Kind;
use crate::effects::{Inst, def};
use crate::randsrc::Rand;
use crate::server::Server;

/// Active effect: the instance of the player effects (S7).
pub type Effect = Inst;

/// Adding to a list; `Some(effect)` if the visible effect changed.
pub fn add(list: &mut Vec<Effect>, n: Effect) -> Option<Effect> {
    match list.iter_mut().find(|e| e.id == n.id) {
        None => {
            list.push(n.clone());
            Some(n)
        }
        Some(e) => e.update(&n).then(|| e.clone()),
    }
}

/// Does the mob accept the effect?
pub fn can_affect(kind: Kind, id: &str) -> bool {
    // F16: the wither refuses every effect (always false).
    if super::wither::refuses_effects(kind) {
        return false;
    }
    // F12: the wither skeleton is immune to the wither effect.
    if super::nether::refuses_effect(kind, id) {
        return false;
    }
    // The silverfish is immune to "infested": poison tag test skipped (like the game).
    if kind == Kind::Silverfish {
        return id != "minecraft:infested";
    }
    if matches!(kind, Kind::Spider | Kind::CaveSpider) && id == "minecraft:poison" {
        return false;
    }
    if kind == Kind::Parched && id == "minecraft:weakness" {
        return false;
    }
    // Slime: immune to "oozing" (game tag, without the magma cube).
    if kind == Kind::Slime {
        return id != "minecraft:oozing";
    }
    // F09: nautiluses are never poisoned.
    if super::sealife::is_nautilus(kind) && id == "minecraft:poison" {
        return false;
    }
    !(kind.undead() && (id == "minecraft:poison" || id == "minecraft:regeneration"))
}

/// Mob attribute touched by an effect modifier.
fn mob_attr<'a>(m: &'a mut Mob, name: &str) -> Option<&'a mut super::mob::Attr> {
    Some(match name {
        "minecraft:movement_speed" => &mut m.attrs.speed,
        "minecraft:attack_damage" => &mut m.attrs.attack,
        "minecraft:max_health" => &mut m.attrs.max_health,
        "minecraft:safe_fall_distance" => &mut m.attrs.safe_fall,
        _ => return None,
    })
}

/// Attribute modifiers of a mob's effects, recomputed (addition, removal, end).
pub fn apply_mob_modifiers(m: &mut Mob) {
    for d in crate::effects::DEFS {
        for (a, id, ..) in d.attrs {
            if let Some(attr) = mob_attr(m, a) {
                attr.remove_mod(id);
            }
        }
    }
    let list = m.x.effects.clone();
    for e in &list {
        let Some(d) = def(&e.id) else { continue };
        for (a, id, per, op) in d.attrs {
            if let Some(attr) = mob_attr(m, a) {
                attr.set_mod(id, per * f64::from(e.amp + 1), *op);
            }
        }
    }
    // Maximum health reduced: health follows.
    let max = m.max_health();
    if m.health > max {
        m.health = max;
    }
}

impl Server {
    /// Effect applied to the player (S7 effects); true if it changed. Mob attacks
    /// do not affect a creative, spectator or dead player.
    pub(crate) fn give_player_effect(&mut self, id: &str, dur: i32, amp: i32) -> bool {
        self.give_player_effect_from(id, dur, amp, None)
    }

    /// Same with the mob that is the source of the effect (`effects_changed` trigger).
    pub(crate) fn give_player_effect_from(&mut self, id: &str, dur: i32, amp: i32, src: Option<&Mob>) -> bool {
        if self.game_mode == 1 || self.game_mode == 3 || self.life.vit.dead {
            return false;
        }
        self.add_effect_from(Inst::new(id, dur, amp), src.map(crate::progress::pred::Who::Mob))
    }

    /// Effect applied to a mob; true if it was accepted and changed.
    pub(crate) fn give_mob_effect(&self, m: &mut Mob, id: &str, dur: i32, amp: i32) -> bool {
        self.add_mob_effect(m, Inst::new(id, dur, amp))
    }

    /// Full instance (`effect` command, effect arrows) applied to a mob.
    pub(crate) fn add_mob_effect(&self, m: &mut Mob, e: Inst) -> bool {
        if !can_affect(m.kind, &e.id) || !m.alive() || def(&e.id).is_none() {
            return false;
        }
        // F15: the dragon refuses every applied effect (it stays "affectable": the instant
        // effects of potions and clouds touch it).
        if super::end::refuses_effects(m.kind) {
            return false;
        }
        if add(&mut m.x.effects, e).is_none() {
            return false;
        }
        apply_mob_modifiers(m);
        self.mob_effect_data(m);
        true
    }

    pub(crate) fn remove_mob_effect(&self, m: &mut Mob, id: &str) -> bool {
        let before = m.x.effects.len();
        m.x.effects.retain(|e| e.id != id);
        if m.x.effects.len() == before {
            return false;
        }
        apply_mob_modifiers(m);
        self.mob_effect_data(m);
        true
    }

    /// All effects of a mob removed; true if there were any.
    pub(crate) fn clear_mob_effects(&self, m: &mut Mob) -> bool {
        if m.x.effects.is_empty() {
            return false;
        }
        m.x.effects.clear();
        apply_mob_modifiers(m);
        self.mob_effect_data(m);
        true
    }

    /// Entity data of the effects: particles (10), ambient (11); invisibility and
    /// glowing go through the flags copied into `sync_data`.
    pub(crate) fn mob_effect_data(&self, m: &mut Mob) {
        use rc_proto_v777::entity::Meta;
        let (parts, ambient) = self.effect_particles(&m.x.effects);
        m.set(10, Meta::Particles(parts));
        m.set(11, Meta::Bool(ambient));
    }

    /// Effects of a mob, at the base tick.
    pub(crate) fn mob_effects_tick(&mut self, m: &mut Mob) {
        if m.x.effects.is_empty() {
            return;
        }
        let ids: Vec<String> = m.x.effects.iter().map(|e| e.id.clone()).collect();
        let mut changed = false;
        for id in ids {
            let Some(mut e) = m.x.effects.iter().find(|x| x.id == id).cloned() else { continue };
            let keep = if !e.remaining() {
                false
            } else {
                let tc = if e.infinite() { m.tick_count } else { e.dur };
                if Self::should_apply(&e.id, tc, e.amp) && !self.mob_effect_tick(m, &e.id, e.amp) {
                    false
                } else {
                    // The effect may have changed during its tick: current state taken back.
                    if let Some(c) = m.x.effects.iter().find(|x| x.id == id) {
                        e = c.clone();
                    }
                    e.tick_down();
                    if e.downgrade() {
                        changed = true;
                    }
                    if let Some(c) = m.x.effects.iter_mut().find(|x| x.id == id) {
                        *c = e.clone();
                    }
                    e.remaining()
                }
            };
            if !keep {
                m.x.effects.retain(|x| x.id != id);
                changed = true;
            }
        }
        if changed {
            apply_mob_modifiers(m);
        }
        // Every tick as long as there are effects (mob read back from a chunk included): the
        // data is only marked if it changes.
        self.mob_effect_data(m);
    }

    /// Effect of a tick on a mob; false if the effect must stop.
    fn mob_effect_tick(&mut self, m: &mut Mob, id: &str, amp: i32) -> bool {
        match id {
            "minecraft:regeneration" => {
                if m.health < m.max_health() {
                    mob_heal(m, 1.0);
                }
            }
            "minecraft:poison" => {
                if m.health > 1.0 {
                    self.mob_hurt(m, "minecraft:magic", 1.0, None, None);
                }
            }
            "minecraft:wither" => {
                self.mob_hurt(m, "minecraft:wither", 1.0, None, None);
            }
            "minecraft:instant_health" | "minecraft:instant_damage" => {
                self.mob_instant(m, id == "minecraft:instant_damage", amp, 1.0);
            }
            _ => {}
        }
        true
    }

    /// Instant healing or damage: inverted for the undead.
    pub(crate) fn mob_instant(&mut self, m: &mut Mob, harm: bool, amp: i32, scale: f64) {
        if harm == m.kind.undead() {
            let n = (scale * f64::from(4i32 << amp.min(30)) + 0.5) as i32;
            mob_heal(m, n.max(0) as f32);
        } else {
            let n = (scale * f64::from(6i32 << amp.min(30)) + 0.5) as i32;
            self.mob_hurt(m, "minecraft:magic", n as f32, None, None);
        }
    }
}

// ---------- Effects of 1.21: hooks at removal (death) and on injury ----------

/// Effect carrier seen by the hooks: player or mob (taken out of the list).
struct Bearer {
    ent: Ent,
    /// Name of the type (cause of the explosion for advancements).
    kind: &'static str,
    pos: [f64; 3],
    /// Height of the box (`f32` like the game).
    height: f32,
    bbox: rc_physics::Aabb,
    pitch: f32,
    yaw: f32,
    player: bool,
}

/// Rotation around the Y axis in single precision, like the game's vector library:
/// sine as a rounded double, cosine deduced from the sine (root) and signed by the quadrant of the angle.
pub(crate) fn rotate_y(v: [f32; 3], angle: f32) -> [f32; 3] {
    let sin = f64::from(angle).sin() as f32;
    let mut cos = f64::from(1.0f32 - sin * sin).sqrt() as f32;
    let tau = std::f64::consts::TAU as f32;
    let a = angle + std::f64::consts::FRAC_PI_2 as f32;
    let mut b = a - (a / tau) as i32 as f32 * tau;
    if b < 0.0 {
        b += tau;
    }
    if b >= std::f64::consts::PI as f32 {
        cos = -cos;
    }
    [v[0] * cos + v[2] * sin, v[1], -v[0] * sin + v[2] * cos]
}

/// Speed of a silverfish coming out of an infested carrier: look × 0.3 (vertical × 1.5 in
/// addition), rotated around Y by an angle drawn in [−π/2, π/2] on the carrier's random.
fn infested_velocity(pitch: f32, yaw: f32, r: &mut dyn Rand) -> [f64; 3] {
    let half = std::f64::consts::FRAC_PI_2 as f32;
    let angle = r.next_float() * (half - -half) + -half;
    let l = rc_physics::view_vector(pitch, yaw).map(|c| c as f32);
    let v = [l[0] * 0.3, l[1] * 0.3 * 1.5, l[2] * 0.3];
    rotate_y(v, angle).map(f64::from)
}

impl Server {
    /// Player carrier: box of their pose. The dead player is never in the "dying" pose on the
    /// server side (their death does not go through that of the living entity): they keep theirs.
    fn player_bearer(&self) -> Bearer {
        let (_, h, _) = self.player_dims();
        Bearer {
            ent: Ent::Player,
            kind: "minecraft:player",
            pos: self.pos,
            height: h as f32,
            bbox: self.player_aabb(),
            pitch: self.pitch,
            yaw: self.yaw,
            player: true,
        }
    }

    fn mob_bearer(m: &Mob) -> Bearer {
        Bearer {
            ent: Ent::Mob(m.id),
            kind: m.kind.info().name,
            pos: m.pos,
            height: m.height(),
            bbox: m.bbox(),
            pitch: m.pitch,
            yaw: m.yaw,
            player: false,
        }
    }

    /// Mob removed at the end of its death: removal effects, then list emptied (no packet).
    pub(crate) fn mob_removed_effects(&mut self, m: &mut Mob) {
        if m.x.effects.is_empty() {
            return;
        }
        let list = std::mem::take(&mut m.x.effects);
        let b = Self::mob_bearer(m);
        self.removed_effects(&list, &b, &mut m.rng);
    }

    /// Player removed (end of the 20 death ticks, or respawn before): same effects; the list
    /// is emptied without a packet (the respawned player restarts without effects).
    pub(crate) fn player_removed_effects(&mut self) {
        if self.life.fx.list.is_empty() {
            return;
        }
        let list = std::mem::take(&mut self.life.fx.list);
        let b = self.player_bearer();
        let mut r = std::mem::replace(&mut self.life.rng, crate::rng::Rng::new(0));
        self.removed_effects(&list, &b, &mut r);
        self.life.rng = r;
    }

    fn removed_effects(&mut self, list: &[Effect], b: &Bearer, r: &mut dyn Rand) {
        for e in list {
            match e.id.as_str() {
                "minecraft:wind_charged" => {
                    let radius = 3.0 + r.next_float() * 2.0;
                    let center = [b.pos[0], b.pos[1] + f64::from(b.height / 2.0), b.pos[2]];
                    self.prog.expl_cause_next = Some(b.kind);
                    let src = super::recent::vibration::Src::Living(b.ent);
                    self.wind_burst_from(src, Some(b.ent), center, radius, 1.0, "minecraft:entity.breeze.wind_burst");
                    self.prog.expl_cause_next = None;
                }
                "minecraft:weaving" if b.player || self.rule("minecraft:mob_griefing", true) => {
                    self.weave_cobwebs(b, r);
                }
                "minecraft:oozing" => self.ooze_slimes(b),
                _ => {}
            }
        }
    }

    /// 2 or 3 cobwebs on 15 cells drawn in the 3×3×3 cube around the carrier: replaceable cell
    /// placed on a sturdy top face, without duplicates; placement (neighbours notified) then event 3018.
    fn weave_cobwebs(&mut self, b: &Bearer, r: &mut dyn Rand) {
        let want = 2 + r.next_int_bound(2) as usize;
        let c = super::mth::block_of(b.pos);
        let mut picked: Vec<[i32; 3]> = Vec::with_capacity(3);
        for _ in 0..15 {
            let x = c[0] - 1 + r.next_int_bound(3);
            let y = c[1] - 1 + r.next_int_bound(3);
            let z = c[2] - 1 + r.next_int_bound(3);
            let p = [x, y, z];
            if !picked.contains(&p)
                && self.v().replaceable(self.st(p))
                && self.v().sturdy(self.st([x, y - 1, z]), crate::blocks::Dir::Up)
            {
                picked.push(p);
                if picked.len() >= want {
                    break;
                }
            }
        }
        let web = self.default_state("minecraft:cobweb");
        for p in picked {
            self.set_block(p, web, 3);
            self.level_event(3018, p, 0);
        }
    }

    /// Two slimes of size 2 half a block above the carrier, minus those that would exceed the
    /// `max_entity_cramming` rule (slimes in the box inflated by 2); rule < 1: no limit.
    fn ooze_slimes(&mut self, b: &Bearer) {
        const REQUESTED: i32 = 2;
        let cram = self.mob_rule_int("minecraft:max_entity_cramming", 24);
        let n = if cram < 1 {
            REQUESTED
        } else {
            let area = rc_physics::Aabb {
                min: [b.bbox.min[0] - 2.0, b.bbox.min[1] - 2.0, b.bbox.min[2] - 2.0],
                max: [b.bbox.max[0] + 2.0, b.bbox.max[1] + 2.0, b.bbox.max[2] + 2.0],
            };
            let near = self
                .mobs
                .list
                .iter()
                .flatten()
                .filter(|o| o.kind == Kind::Slime && !o.removed && Ent::Mob(o.id) != b.ent && o.bbox().intersects(&area))
                .take(cram as usize)
                .count() as i32;
            (cram - near).clamp(0, REQUESTED)
        };
        let pos = [b.pos[0], b.pos[1] + 0.5, b.pos[2]];
        for _ in 0..n {
            let mut c = self.new_mob(Kind::Slime, pos, 0.0);
            c.head_yaw = 0.0;
            c.body_yaw = 0.0;
            c.set_size(2, true);
            c.pos = pos;
            c.old = pos;
            c.yaw = self.mobs.rng.next_float() * 360.0;
            c.pitch = 0.0;
            self.add_mob(c);
        }
    }

    /// Mob hurt (hit that landed): "infested" effect.
    pub(crate) fn mob_hurt_effects(&mut self, m: &mut Mob) {
        if !m.x.effects.iter().any(|e| e.id == "minecraft:infested") {
            return;
        }
        let b = Self::mob_bearer(m);
        self.infest(&b, &mut m.rng);
    }

    /// Player hurt (hit that landed): "infested" effect.
    pub(crate) fn player_hurt_effects(&mut self) {
        if !self.life.fx.has("minecraft:infested") {
            return;
        }
        let b = self.player_bearer();
        let mut r = std::mem::replace(&mut self.life.rng, crate::rng::Rng::new(0));
        self.infest(&b, &mut r);
        self.life.rng = r;
    }

    /// One time in ten (draw ≤ 0.1): 1 or 2 silverfish thrown from the middle of the
    /// carrier in the direction of their look, each with the hurt sound of the species.
    fn infest(&mut self, b: &Bearer, r: &mut dyn Rand) {
        if r.next_float() > 0.1 {
            return;
        }
        let count = 1 + r.next_int_bound(2);
        let pos = [b.pos[0], b.pos[1] + f64::from(b.height) / 2.0, b.pos[2]];
        for _ in 0..count {
            let vel = infested_velocity(b.pitch, b.yaw, r);
            let mut s = self.new_mob(Kind::Silverfish, pos, 0.0);
            s.head_yaw = 0.0;
            s.body_yaw = 0.0;
            s.pos = pos;
            s.old = pos;
            s.yaw = self.mobs.rng.next_float() * 360.0;
            s.pitch = 0.0;
            s.vel = vel;
            self.mob_sound(&s, "minecraft:entity.silverfish.hurt", 1.0, 1.0);
            self.add_mob(s);
        }
    }
}

/// `heal` of a mob: only if alive, capped at its maximum health.
fn mob_heal(m: &mut Mob, amount: f32) {
    if m.health > 0.0 {
        m.health = (m.health + amount).clamp(0.0, m.max_health());
    }
}
