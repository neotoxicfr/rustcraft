//! Warden (`server-mobs.md` § 4.F14): brain (emerge, dig, roar, fight,
//! investigate, sniff, idle), anger per suspect (150 at most, −1 per
//! second, players and anger first), vibration listening (radius 16, cooldown 40), sniffing
//! of nearby entities, sonic boom (damage 10, knockback 2.5 / 0.5), darkness (20 blocks every
//! 6 s), invulnerable while digging or emerging, disappears while digging.

use rc_proto::nbt::Nbt;

use super::super::brainy::brain::{Act, Bk, Brain, K, Sn, Spd, St, Track, act, at, core, move_sink, run_one, seq};
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use super::vibration::{Src, VibData, VibInfo, Who};
use super::xbrain::{Stop, XBk, XK, XSn};
use super::{POSE_DIGGING, POSE_EMERGING, POSE_ROARING, POSE_SNIFFING, POSE_STANDING};
use crate::nbtio;
use crate::server::Server;

/// The ceiling of 133.59999.
pub const EMERGE_DURATION: i32 = 134;
pub const DIGGING_DURATION: i32 = 100;
pub const ROAR_DURATION: i32 = 84;
/// The ceiling of 83.2.
pub const SNIFFING_DURATION: i32 = 84;
pub const SONIC_DURATION: i32 = 60;
/// Sonic boom sound: 34 ticks after the start.
const SONIC_SOUND_DELAY: i64 = 34;
const MAX_ANGER: i32 = 150;

// ---------- Anger (`AngerLevel`, anger management) ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngerLevel {
    Calm,
    Agitated,
    Angry,
}

impl AngerLevel {
    pub fn by_anger(a: i32) -> AngerLevel {
        if a >= 80 {
            AngerLevel::Angry
        } else if a >= 40 {
            AngerLevel::Agitated
        } else {
            AngerLevel::Calm
        }
    }

    pub fn angry(self) -> bool {
        self == AngerLevel::Angry
    }

    fn ambient(self) -> &'static str {
        match self {
            AngerLevel::Calm => "minecraft:entity.warden.ambient",
            AngerLevel::Agitated => "minecraft:entity.warden.agitated",
            AngerLevel::Angry => "minecraft:entity.warden.angry",
        }
    }

    fn listening(self) -> &'static str {
        match self {
            AngerLevel::Calm => "minecraft:entity.warden.listening",
            _ => "minecraft:entity.warden.listening_angry",
        }
    }
}

/// Anger per suspect: loaded suspects (sorted) and anger kept by UUID (read back).
#[derive(Debug, Clone)]
pub struct Anger {
    /// Loaded suspect: entity, UUID, anger.
    pub suspects: Vec<(Ent, u128, i32)>,
    pub by_uuid: Vec<(u128, i32)>,
    pub highest: i32,
    pub conversion_delay: i32,
}

impl Default for Anger {
    fn default() -> Anger {
        // Initial conversion delay drawn on a local unseeded random (0 to 2).
        let mut r = JRand::from_time(0x5741_5244);
        Anger { suspects: Vec::new(), by_uuid: Vec::new(), highest: 0, conversion_delay: r.next_int_bound(3) }
    }
}

impl Anger {
    fn anger_of(&self, e: Ent) -> i32 {
        self.suspects.iter().find(|s| s.0 == e).map_or(0, |s| s.2)
    }

    /// Angry first, then the player, then decreasing anger
    /// (stable sort); the strongest anger compared becomes the highest.
    fn sort(&mut self) {
        self.highest = 0;
        if self.suspects.len() == 1 {
            self.highest = self.suspects[0].2;
            return;
        }
        self.highest = self.suspects.iter().map(|s| s.2).max().unwrap_or(0);
        self.suspects.sort_by(|a, b| {
            let (aa, ab) = (AngerLevel::by_anger(a.2).angry(), AngerLevel::by_anger(b.2).angry());
            if aa != ab {
                return if aa { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
            }
            let (pa, pb) = (a.0 == Ent::Player, b.0 == Ent::Player);
            if pa != pb {
                return if pa { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
            }
            b.2.cmp(&a.2)
        });
    }
}

// ---------- Brain ----------

pub fn brain(rng: &mut JRand) -> Brain {
    use St::*;
    use XK::*;
    let kx = K::X;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![Bk::Swim { chance: 0.8 }, Bk::X(XBk::WardenLook), Bk::LookSink { lo: 45, hi: 90 }, move_sink()],
    );
    let emerge = seq(5, Act::Emerge, vec![Bk::X(XBk::Emerging)]);
    let dig = vec![at(0, Act::Dig, Bk::X(XBk::ForceUnmount)), at(1, Act::Dig, Bk::X(XBk::Digging))];
    let idle = seq(
        10,
        Act::Idle,
        vec![
            Bk::X(XBk::SetRoarTarget),
            Bk::X(XBk::TryToSniff),
            run_one(
                &[(kx(IsSniffing), Absent)],
                0x5741_5244,
                vec![(Bk::Stroll { speed: 0.5, from_water: true }, 2), (Bk::DoNothing { lo: 30, hi: 60 }, 1)],
            ),
        ],
    );
    let roar = seq(10, Act::Roar, vec![Bk::X(XBk::Roar)]);
    let fight = seq(
        10,
        Act::Fight,
        vec![
            Bk::X(XBk::DigCooldown),
            Bk::X(XBk::StopAttack { why: Stop::Warden, positives: 0 }),
            Bk::X(XBk::LookAtTarget),
            Bk::WalkToAttack(Spd::Fixed(1.2)),
            Bk::X(XBk::SonicBoom),
            Bk::Melee { cd: 18 },
        ],
    );
    let investigate = seq(5, Act::Investigate, vec![Bk::X(XBk::SetRoarTarget), Bk::X(XBk::GoToDisturbance)]);
    let sniff = seq(5, Act::Sniff, vec![Bk::X(XBk::SetRoarTarget), Bk::X(XBk::Sniffing)]);
    Brain::new(
        rng,
        &[Sn::Players, Sn::X(XSn::WardenEntity)],
        vec![
            (core(), core_nodes),
            (act(Act::Emerge, &[(kx(IsEmerging), Present)], &[kx(IsEmerging)]), emerge),
            (act(Act::Dig, &[(kx(RoarTarget), Absent), (kx(DigCooldown), Absent)], &[]), dig),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Roar, &[(kx(RoarTarget), Present)], &[kx(RoarTarget)]), roar),
            (act(Act::Fight, &[(K::Attack, Present)], &[K::Attack]), fight),
            (act(Act::Investigate, &[(kx(Disturbance), Present)], &[kx(Disturbance)]), investigate),
            (act(Act::Sniff, &[(kx(IsSniffing), Present)], &[kx(IsSniffing)]), sniff),
        ],
    )
}

pub fn on_new(_m: &mut Mob) {}

// ---------- Tools ----------

pub fn digging_or_emerging(m: &Mob) -> bool {
    let p = super::super::brainy::pose(m);
    p == POSE_DIGGING || p == POSE_EMERGING
}

fn set_pose(m: &mut Mob, p: i32) {
    super::super::brainy::set_pose(m, p);
}

fn sound(s: &mut Server, m: &mut Mob, name: &str, volume: f32) {
    s.mob_sound(m, name, volume, 1.0);
}

/// Living, neither creative nor spectator, not another warden nor an armor stand,
/// not dead.
pub fn can_target(s: &Server, _m: &Mob, e: Ent) -> bool {
    match e {
        Ent::Player => !matches!(s.game_mode, 1 | 3) && s.ent_alive(Ent::Player),
        Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| o.alive() && o.kind != Kind::Warden),
    }
}

/// Active anger level (the active anger).
pub fn anger_level(m: &Mob) -> AngerLevel {
    AngerLevel::by_anger(active_anger(m))
}

fn active_anger(m: &Mob) -> i32 {
    match m.target {
        None => m.rec.anger.highest,
        Some(t) => m.rec.anger.anger_of(t),
    }
}

/// First targetable suspect, only when angry.
fn entity_angry_at(s: &Server, m: &Mob) -> Option<Ent> {
    if !anger_level(m).angry() {
        return None;
    }
    m.rec.anger.suspects.iter().map(|x| x.0).find(|&e| can_target(s, m, e))
}

/// Cooldown relaunched only if it is already running.
fn set_dig_cooldown(b: &mut Brain) {
    if b.mem.x.dig_cooldown.has() {
        b.mem.x.dig_cooldown.set_exp((), 1200);
    }
}

/// The anger increase.
fn increase_anger_at(s: &mut Server, m: &mut Mob, b: &mut Brain, e: Option<Ent>, amount: i32, play: bool) {
    let Some(e) = e else { return };
    if m.no_ai || !can_target(s, m, e) {
        return;
    }
    set_dig_cooldown(b);
    let maybe_switch = m.target != Some(Ent::Player);
    let a = &mut m.rec.anger;
    let new = !a.suspects.iter().any(|x| x.0 == e);
    let mut current = (a.anger_of(e) + amount).min(MAX_ANGER);
    if new {
        let uuid = match e {
            Ent::Player => s.uuid,
            Ent::Mob(id) => s.mob_ref(id).map_or(0, |o| o.uuid),
        };
        if let Some(i) = a.by_uuid.iter().position(|x| x.0 == uuid) {
            // The anger read back is added without a cap.
            current += a.by_uuid.remove(i).1;
        }
        a.suspects.push((e, uuid, current));
    } else if let Some(x) = a.suspects.iter_mut().find(|x| x.0 == e) {
        x.2 = current;
    }
    a.sort();
    if e == Ent::Player && maybe_switch && AngerLevel::by_anger(current).angry() {
        b.mem.attack.erase();
        m.target = None;
    }
    if play {
        listening_sound(s, m);
    }
}

fn listening_sound(s: &mut Server, m: &mut Mob) {
    if super::super::brainy::pose(m) != POSE_ROARING {
        let snd = anger_level(m).listening();
        let p = Server::voice_pitch(m);
        s.mob_sound(m, snd, 10.0, p);
    }
}

/// Roar forgotten, target set, sonic boom in 200 ticks.
fn set_attack_target(m: &mut Mob, b: &mut Brain, t: Ent) {
    b.mem.x.roar_target.erase();
    b.mem.attack.set(t);
    b.mem.cant_reach.erase();
    b.mem.x.sonic_cooldown.set_exp((), 200);
    m.target = Some(t);
}

/// Only without anger or target.
fn set_disturbance(s: &Server, m: &Mob, b: &mut Brain, p: [i32; 3]) {
    if entity_angry_at(s, m).is_none() && !b.mem.attack.has() {
        set_dig_cooldown(b);
        b.mem.x.sniff_cooldown.set_exp((), 100);
        b.mem.look.set_exp(Track::block(p), 100);
        b.mem.x.disturbance.set_exp(p, 100);
        b.mem.walk.erase();
    }
}

/// The application of darkness around the warden / the application of an effect to nearby players: darkness 13 s
/// (without particles) to players in survival or adventure within the radius, unless they already have it
/// for more than 10 s.
pub fn apply_darkness_around(s: &mut Server, pos: [f64; 3], _source: Option<Ent>, radius: f64) {
    if !matches!(s.game_mode, 0 | 2) || !s.ent_alive(Ent::Player) {
        return;
    }
    let d2 = (0..3).map(|i| (s.pos[i] - pos[i]).powi(2)).sum::<f64>();
    if d2 >= radius * radius {
        return;
    }
    let ok = match s.fx().get("minecraft:darkness") {
        None => true,
        Some(e) => e.amp < 0 || (e.dur != -1 && e.dur <= 199),
    };
    if ok {
        let mut e = crate::effects::Inst::new("minecraft:darkness", 260, 0);
        e.visible = false;
        s.add_effect(e);
    }
}

// ---------- Tick ----------

/// The warden's tick (server): vibrations, digging cooldown maintained if persistent,
/// listener moved with the mob (eyes).
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    s.listener_move(Who::Mob(m.id), [m.pos[0], m.eye_y(), m.pos[2]]);
    s.vib_tick_mob(m);
    if (m.persistent || m.mount.vehicle.is_some())
        && let Some(b) = m.bm.brain.as_mut()
    {
        set_dig_cooldown(b);
    }
}

/// Brain, darkness every 120 ticks, anger every second,
/// activity.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    s.bz_brain_tick(m, b);
    m.target = b.mem.attack.get();
    if (m.tick_count + m.id) % 120 == 0 {
        apply_darkness_around(s, m.pos, Some(Ent::Mob(m.id)), 20.0);
    }
    if m.tick_count % 20 == 0 {
        anger_tick(s, m);
        m.rec.client_anger = active_anger(m);
    }
    b.first_valid(&[Act::Emerge, Act::Dig, Act::Roar, Act::Fight, Act::Investigate, Act::Sniff, Act::Idle]);
}

/// UUID conversion every 2 calls, −1 everywhere, invalid suspects
/// forgotten.
fn anger_tick(s: &Server, m: &mut Mob) {
    let mut a = std::mem::take(&mut m.rec.anger);
    a.conversion_delay -= 1;
    if a.conversion_delay <= 0 {
        let mut kept = Vec::new();
        for (u, v) in std::mem::take(&mut a.by_uuid) {
            let e = if u == s.uuid { Some(Ent::Player) } else { s.mobs.get_uuid(u).map(|o| Ent::Mob(o.id)) };
            match e {
                Some(e) => a.suspects.push((e, u, v)),
                None => kept.push((u, v)),
            }
        }
        a.by_uuid = kept;
        a.conversion_delay = 2;
    }
    a.by_uuid.retain_mut(|x| {
        if x.1 <= 1 {
            false
        } else {
            x.1 -= 1;
            true
        }
    });
    a.suspects.retain_mut(|x| {
        let valid = can_target(s, m, x.0);
        if x.2 > 1 && valid {
            x.2 -= 1;
            true
        } else {
            false
        }
    });
    a.sort();
    m.rec.anger = a;
}

/// Ambient sound by anger (none while roaring, digging or emerging).
pub fn ambient_sound(m: &Mob) -> Option<&'static str> {
    let p = super::super::brainy::pose(m);
    (p != POSE_ROARING && !digging_or_emerging(m)).then(|| anger_level(m).ambient())
}

/// Digging cooldown 1,200; triggered (shrieker): emergence (agitated sound).
pub fn finalize(s: &mut Server, m: &mut Mob, reason: super::super::spawn::Reason) {
    if let Some(b) = m.bm.brain.as_mut() {
        b.mem.x.dig_cooldown.set_exp((), 1200);
    }
    if reason == super::super::spawn::Reason::Triggered {
        set_pose(m, POSE_EMERGING);
        if let Some(b) = m.bm.brain.as_mut() {
            b.mem.x.is_emerging.set_exp((), i64::from(EMERGE_DURATION));
        }
        sound(s, m, "minecraft:entity.warden.agitated", 5.0);
    }
}

/// Spawn data: 1 if the warden emerges.
pub fn add_entity_data(m: &Mob) -> i32 {
    i32::from(super::super::brainy::pose(m) == POSE_EMERGING)
}

// ---------- Combat ----------

/// Invulnerable while digging or emerging (except `bypasses_invulnerability`).
pub fn before_hurt(s: &mut Server, m: &mut Mob, ty: &str) -> Option<bool> {
    (digging_or_emerging(m) && !s.dmg_tag(ty, "minecraft:bypasses_invulnerability")).then_some(false)
}

/// After the server-side damage computation (even without damage): anger 100 against the author; target taken if they
/// hit directly or are within 5 blocks.
pub fn after_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) {
    if m.no_ai || digging_or_emerging(m) {
        return;
    }
    let Some(mut b) = m.bm.brain.take() else { return };
    increase_anger_at(s, m, &mut b, by, 100, false);
    if !b.mem.attack.has()
        && let Some(a) = by
    {
        let direct =
            s.mobs.rec.direct_source.unwrap_or_else(|| {
                !s.dmg_tag(ty, "minecraft:is_projectile") && !s.dmg_tag(ty, "minecraft:is_explosion")
            });
        let close = s.ent_pos(a).is_some_and(|p| super::super::mth::dist2(p, m.pos) < 25.0);
        if direct || close {
            set_attack_target(m, &mut b, a);
        }
    }
    m.bm.brain = Some(b);
}

/// Event 4, impact sound (10), sonic boom pushed back by 40 ticks.
pub fn before_melee(s: &mut Server, m: &mut Mob) {
    s.mob_event(m, 4);
    let p = Server::voice_pitch(m);
    s.mob_sound(m, "minecraft:entity.warden.attack_impact", 10.0, p);
    if let Some(b) = m.bm.brain.as_mut() {
        b.mem.x.sonic_cooldown.set_exp((), 40);
    }
}

/// Pushed by an entity: anger and investigation, once per second.
pub fn on_push(s: &mut Server, m: &mut Mob, by: Ent) {
    if m.no_ai {
        return;
    }
    let Some(mut b) = m.bm.brain.take() else { return };
    if !b.mem.x.touch_cooldown.has() {
        b.mem.x.touch_cooldown.set_exp((), 20);
        increase_anger_at(s, m, &mut b, Some(by), 35, true);
        if let Some(p) = s.ent_pos(by) {
            let bp = p.map(|v| v.floor() as i32);
            set_disturbance(s, m, &mut b, bp);
        }
    }
    m.bm.brain = Some(b);
}

/// Anger erased if the target is no longer targetable; digging cooldown.
pub fn on_target_invalid(s: &Server, m: &mut Mob, b: &mut Brain, target: Ent) {
    if !can_target(s, m, target) {
        m.rec.anger.suspects.retain(|x| x.0 != target);
        m.rec.anger.sort();
    }
    set_dig_cooldown(b);
}

// ---------- Behaviors ----------

/// Looks at the roar target, otherwise the disturbance.
pub fn look_trigger(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    let _ = m;
    let target = b
        .mem
        .x
        .roar_target
        .get()
        .and_then(|e| s.ent_pos(e))
        .map(|p| p.map(|v| v.floor() as i32))
        .or_else(|| b.mem.x.disturbance.get());
    let Some(p) = target else { return false };
    b.mem.look.set(Track::block(p));
    true
}

/// `SetRoarTarget(the entity targeted by anger)`.
pub fn set_roar_target(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    let Some(t) = entity_angry_at(s, m) else { return false };
    if !can_target(s, m, t) {
        return false;
    }
    b.mem.x.roar_target.set(t);
    b.mem.cant_reach.erase();
    true
}

/// `TryToSniff`: sniffs (cooldown 100 to 200, world random).
pub fn try_to_sniff(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    b.mem.x.is_sniffing.set(());
    let cd = s.mobs.rng.int_between(100, 200);
    b.mem.x.sniff_cooldown.set_exp((), i64::from(cd));
    b.mem.walk.erase();
    set_pose(m, POSE_SNIFFING);
    true
}

pub fn emerging_start(s: &mut Server, m: &mut Mob) {
    set_pose(m, POSE_EMERGING);
    sound(s, m, "minecraft:entity.warden.emerge", 5.0);
}

/// On the ground, placement and sound; otherwise (in a liquid) agitated sound and immediate
/// disappearance.
pub fn digging_start(s: &mut Server, m: &mut Mob, _b: &mut Brain, _t: i64, _end: i64) {
    if m.on_ground {
        set_pose(m, POSE_DIGGING);
        sound(s, m, "minecraft:entity.warden.dig", 5.0);
    } else {
        sound(s, m, "minecraft:entity.warden.agitated", 5.0);
        m.removed = true;
    }
}

pub fn roar_start(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    b.mem.x.roar_sound_delay.set_exp((), 25);
    b.mem.walk.erase();
    let Some(t) = b.mem.x.roar_target.get() else { return };
    b.mem.look.set(Track::ent(t, true));
    set_pose(m, POSE_ROARING);
    increase_anger_at(s, m, b, Some(t), 20, false);
}

pub fn roar_tick(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if !b.mem.x.roar_sound_delay.has() && !b.mem.x.roar_sound_cooldown.has() {
        b.mem.x.roar_sound_cooldown.set_exp((), i64::from(ROAR_DURATION - 25));
        sound(s, m, "minecraft:entity.warden.roar", 3.0);
    }
}

pub fn roar_stop(_s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if super::super::brainy::pose(m) == POSE_ROARING {
        set_pose(m, POSE_STANDING);
    }
    if let Some(t) = b.mem.x.roar_target.get() {
        set_attack_target(m, b, t);
    }
    b.mem.x.roar_target.erase();
}

pub fn sniffing_start(s: &mut Server, m: &mut Mob) {
    sound(s, m, "minecraft:entity.warden.sniff", 5.0);
}

/// Anger against the sniffed entity at ±6/20, disturbance at its position.
pub fn sniffing_stop(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if super::super::brainy::pose(m) == POSE_SNIFFING {
        set_pose(m, POSE_STANDING);
    }
    b.mem.x.is_sniffing.erase();
    let Some(e) = b.mem.attackable.get().filter(|&e| can_target(s, m, e)) else { return };
    let Some(p) = s.ent_pos(e) else { return };
    if super::closer_than_xz_y(m.pos, p, 6.0, 20.0) {
        increase_anger_at(s, m, b, Some(e), 35, true);
    }
    if !b.mem.x.disturbance.has() {
        set_disturbance(s, m, b, p.map(|v| v.floor() as i32));
    }
}

/// Attack at cooldown 60, sound in 34, event 62, charge sound.
pub fn sonic_start(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    b.mem.attack_cooling.set_exp((), i64::from(SONIC_DURATION));
    b.mem.x.sonic_sound_delay.set_exp((), SONIC_SOUND_DELAY);
    s.mob_event(m, 62);
    sound(s, m, "minecraft:entity.warden.sonic_charge", 3.0);
}

/// Gaze on the target; at the sound deadline, boom (particles along the
/// chest → eyes line of the target), damage 10 and push.
pub fn sonic_tick(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if let Some(t) = b.mem.attack.get()
        && let Some(p) = s.ent_pos(t)
    {
        let (y, x) = (10.0, super::super::goals::max_head_x(m));
        s.look_at(m, p, y, x);
    }
    if b.mem.x.sonic_sound_delay.has() || b.mem.x.sonic_sound_cooldown.has() {
        return;
    }
    b.mem.x.sonic_sound_cooldown.set_exp((), i64::from(SONIC_DURATION) - SONIC_SOUND_DELAY);
    let Some(t) = b.mem.attack.get().filter(|&t| can_target(s, m, t)) else { return };
    let (Some(tp), Some(eye)) = (s.ent_pos(t), s.ent_eye(t)) else { return };
    if !super::closer_than_xz_y(m.pos, tp, 15.0, 20.0) {
        return;
    }
    let src = [m.pos[0], m.pos[1] + f64::from(1.6f32), m.pos[2]];
    let d = [tp[0] - src[0], eye - src[1], tp[2] - src[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let n = if l < 1.0e-5 { [0.0; 3] } else { d.map(|v| v / l) };
    let steps = l.floor() as i32 + 7;
    for i in 1..steps {
        let pp = [src[0] + n[0] * f64::from(i), src[1] + n[1] * f64::from(i), src[2] + n[2] * f64::from(i)];
        s.particles("minecraft:sonic_boom", pp, [0.0; 3], 0.0, 1);
    }
    sound(s, m, "minecraft:entity.warden.sonic_boom", 3.0);
    let hurt = match t {
        // Default knockback from the warden (direct source), then the boom's push.
        Ent::Player => {
            s.player_hurt_by_mob(m, "minecraft:sonic_boom", 10.0, Some((m.pos[0] - tp[0], m.pos[2] - tp[2])))
        }
        Ent::Mob(id) => {
            let Some(i) = s.mob_index(id) else { return };
            let Some(mut o) = s.mobs.list[i].take() else { return };
            let r = s.mob_hurt_by(&mut o, m, "minecraft:sonic_boom", 10.0);
            if r {
                let res = o.attrs.knockback_resistance.value();
                let (kv, kh) = (0.5 * (1.0 - res), 2.5 * (1.0 - res));
                o.vel = [o.vel[0] + n[0] * kh, o.vel[1] + n[1] * kv, o.vel[2] + n[2] * kh];
                o.needs_sync = true;
            }
            s.mobs.list[i] = Some(o);
            return;
        }
    };
    if hurt {
        let res = s.player_attribute("minecraft:knockback_resistance");
        let (kv, kh) = (0.5 * (1.0 - res), 2.5 * (1.0 - res));
        s.player_push([n[0] * kh, n[1] * kv, n[2] * kh]);
    }
}

// ---------- Vibrations ----------

/// Active, alive, outside cooldown, neither digging nor emerging; a living
/// source must be targetable.
pub fn can_receive_vibration(s: &Server, w: &Mob, src: Option<Src>) -> bool {
    let cooldown = w.bm.brain.as_ref().is_some_and(|b| b.mem.x.vibration_cooldown.has());
    if w.no_ai || w.dead || w.health <= 0.0 || cooldown || digging_or_emerging(w) {
        return false;
    }
    match src.and_then(Src::living) {
        Some(e) => can_target(s, w, e),
        None => true,
    }
}

/// Cooldown 40, event 61, clicking; anger against the source (or
/// the shooter, 10 the first time, 35 if a recent projectile); disturbance if not angry.
pub fn on_receive_vibration(s: &mut Server, m: &mut Mob, info: VibInfo, owner: Option<Ent>, _data: &VibData) {
    if m.dead || m.health <= 0.0 {
        return;
    }
    let Some(mut b) = m.bm.brain.take() else { return };
    b.mem.x.vibration_cooldown.set_exp((), 40);
    s.mob_event(m, 61);
    let p = Server::voice_pitch(m);
    s.mob_sound(m, "minecraft:entity.warden.tendril_clicks", 5.0, p);
    let origin = info.pos.map(|v| v.floor() as i32);
    let mut suspicious = origin;
    let src = s.vib_source(&info).and_then(Src::living);
    if let Some(o) = owner {
        if s.ent_pos(o).is_some_and(|op| super::super::mth::dist2(op, m.pos) < 900.0) {
            if b.mem.x.recent_projectile.has() {
                if can_target(s, m, o)
                    && let Some(op) = s.ent_pos(o)
                {
                    suspicious = op.map(|v| v.floor() as i32);
                }
                increase_anger_at(s, m, &mut b, Some(o), 35, true);
            } else {
                increase_anger_at(s, m, &mut b, Some(o), 10, true);
            }
        }
        b.mem.x.recent_projectile.set_exp((), 100);
    } else {
        increase_anger_at(s, m, &mut b, src, 35, true);
    }
    if !anger_level(m).angry() {
        let active = m.rec.anger.suspects.iter().map(|x| x.0).find(|&e| can_target(s, m, e));
        if owner.is_some() || active.is_none() || active == src {
            set_disturbance(s, m, &mut b, suspicious);
        }
    }
    m.bm.brain = Some(b);
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    let mut a = Anger::default();
    if let Some(Nbt::List(list)) = e.get("anger").and_then(|x| x.get("suspects")) {
        for s in list {
            let uuid = match s.get("uuid") {
                Some(Nbt::IntArray(v)) if v.len() == 4 => {
                    v.iter().fold(0u128, |acc, &x| (acc << 32) | u128::from(x as u32))
                }
                _ => continue,
            };
            let anger = nbtio::int(s, "anger").unwrap_or(0) as i32;
            a.by_uuid.push((uuid, anger.max(0)));
        }
    }
    m.rec.anger = a;
    m.rec.client_anger = active_anger(m);
    m.rec.vib = VibData::load(e.get("listener"));
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let ints = |u: u128| Nbt::IntArray(vec![(u >> 96) as i32, (u >> 64) as i32, (u >> 32) as i32, u as i32]);
    // Loaded suspects, then anger kept by UUID.
    let pairs = m.rec.anger.suspects.iter().map(|x| (x.1, x.2)).chain(m.rec.anger.by_uuid.iter().copied());
    let list: Vec<Nbt> =
        pairs.map(|(u, v)| Nbt::Compound(vec![("uuid".into(), ints(u)), ("anger".into(), Nbt::Int(v))])).collect();
    nbtio::set(e, "anger", Nbt::Compound(vec![("suspects".into(), Nbt::List(list))]));
    nbtio::set(e, "listener", m.rec.vib.save());
}
