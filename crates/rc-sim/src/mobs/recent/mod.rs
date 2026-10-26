//! Recent mobs (family F14, `server-mobs.md` § 4.F14): warden, breeze, creaking
//! and happy ghast. Info sheets, own state, metadata, NBT, brains (extension of the F10 brain
//! in `xbrain.rs`) and hooks called by the common engine (`mobs/`). Each species has its
//! file; so do the related blocks: creaking heart (`heart.rs`), sculk shrieker and
//! vibrations (`vibration.rs`), dried ghast (`dried_ghast.rs`), wind charges (`wind.rs`).

pub(crate) mod breeze;
pub(crate) mod creaking;
pub(crate) mod dried_ghast;
pub(crate) mod happy_ghast;
pub(crate) mod heart;
pub(crate) mod shrieker;
pub(crate) mod vibration;
pub(crate) mod warden;
pub(crate) mod wind;
pub(crate) mod xbrain;

#[cfg(test)]
mod event_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::mob::{Ent, Mob};
use super::nav::PathType;
use super::species::{Category, Info, Kind, info as base_info};
use crate::server::Server;

// ---------- Poses (`Pose`) ----------

pub const POSE_STANDING: i32 = 0;
pub const POSE_LONG_JUMPING: i32 = 6;
pub const POSE_ROARING: i32 = 11;
pub const POSE_SNIFFING: i32 = 12;
pub const POSE_EMERGING: i32 = 13;
pub const POSE_DIGGING: i32 = 14;
pub const POSE_SLIDING: i32 = 15;
pub const POSE_SHOOTING: i32 = 16;
pub const POSE_INHALING: i32 = 17;

// ---------- Info sheets ----------

/// Monster: experience 5, tracking 8, absent in peaceful.
const fn monster(name: &'static str, width: f32, height: f32, eye: f32) -> Info {
    let mut i = base_info(name, Category::Monster, width, height, eye);
    i.xp = 5;
    i.tracking = 8;
    i.not_in_peaceful = true;
    i.hurt = "minecraft:entity.hostile.hurt";
    i.death = "minecraft:entity.hostile.death";
    i
}

/// Warden: 0.9 × 2.9 (eyes at 0.85 × height), HP 500, speed 0.3, damage 30, tracking 24,
/// knockback resistance 1 and attack knockback 1.5 (set at construction), volume 4, client
/// tracking 16, fire-immune. Ambient sound by anger (computed).
static WARDEN: Info = {
    let mut i = monster("minecraft:warden", 0.9, 2.9, 2.9 * 0.85);
    i.health = 500.0;
    i.speed = 0.3f32 as f64;
    i.attack = 30.0;
    i.follow = 24.0;
    i.tracking = 16;
    i.volume = 4.0;
    i.hurt = "minecraft:entity.warden.hurt";
    i.death = "minecraft:entity.warden.death";
    i.fire_immune = true;
    i
};

/// Breeze: 0.6 × 1.77 (eyes at 1.3452), base attributes of a mob: speed 0.63, HP 30,
/// tracking 24, damage 3; 10 experience, client tracking 10. Ambient sound
/// played only client-side: nothing is sent by the server.
static BREEZE: Info = {
    let mut i = monster("minecraft:breeze", 0.6, 1.77, 1.3452);
    i.speed = 0.63f32 as f64;
    i.health = 30.0;
    i.follow = 24.0;
    i.attack = 3.0;
    i.xp = 10;
    i.tracking = 10;
    i.hurt = "minecraft:entity.breeze.hurt";
    i.death = "minecraft:entity.breeze.death";
    // `#fall_damage_immune` (the landing sound is still played by its hook).
    i.fall_immune = true;
    i
};

/// Creaking: 0.9 × 2.7 (eyes at 2.3), HP 1, speed 0.4, damage 3, tracking 32, step 1.0625, no
/// experience. Ambient sound only when inactive (computed).
static CREAKING: Info = {
    let mut i = monster("minecraft:creaking", 0.9, 2.7, 2.3);
    i.health = 1.0;
    i.speed = 0.4f32 as f64;
    i.attack = 3.0;
    i.follow = 32.0;
    i.step = 1.0625;
    i.xp = 0;
    i.death = "minecraft:entity.creaking.death";
    i
};

/// Happy ghast: animal 4 × 4 (eyes at 2.6), HP 20, speed 0.05 (flight 0.05), tracking 16,
/// tempting 16, client tracking 10, no fall damage. Volume and sounds by age.
static HAPPY_GHAST: Info = {
    let mut i = super::species::animal("minecraft:happy_ghast", 4.0, 4.0, 2.6, 20.0, 0.05);
    i.follow = 16.0;
    i.tracking = 10;
    i.fall_immune = true;
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Warden => &WARDEN,
        Kind::Breeze => &BREEZE,
        Kind::Creaking => &CREAKING,
        _ => &HAPPY_GHAST,
    }
}

pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::Warden | Kind::Breeze | Kind::Creaking | Kind::HappyGhast)
}

/// The game's animal (happy ghast).
pub fn is_animal(k: Kind) -> bool {
    k == Kind::HappyGhast
}

/// Food (`happy_ghast_food`: snowballs).
pub fn food_tag(k: Kind) -> Option<&'static str> {
    (k == Kind::HappyGhast).then_some("minecraft:happy_ghast_food")
}

/// Ghastling: scale 0.2375 (eyes at 0.46875).
pub fn baby_dims(k: Kind) -> Option<(f32, f32, f32)> {
    (k == Kind::HappyGhast).then_some((4.0 * happy_ghast::BABY_SCALE, 4.0 * happy_ghast::BABY_SCALE, 0.468_75))
}

/// Own dimensions by pose: buried warden (height 1 while digging or emerging).
pub fn pose_dims(m: &Mob) -> Option<(f32, f32, f32)> {
    if m.kind == Kind::Warden && warden::digging_or_emerging(m) {
        let i = info(Kind::Warden);
        return Some((i.width, 1.0, 0.85));
    }
    None
}

// ---------- Tools ----------

/// Is the effect active on the mob?
pub fn has_effect(m: &Mob, id: &str) -> bool {
    m.x.effects.iter().any(|e| e.id == id)
}

/// Horizontal and vertical distance (strict bounds).
pub fn closer_than_xz_y(a: [f64; 3], b: [f64; 3], xz: f64, y: f64) -> bool {
    let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    dx * dx + dz * dz < xz * xz && dy * dy < y * y
}

/// Day timeline (`creaking_active`): active from 12,600 to 23,400 in the normal world.
pub fn creaking_active(s: &Server) -> bool {
    if s.dim.name != "minecraft:overworld" {
        return false;
    }
    let t = s.day_time().rem_euclid(24000);
    (12600..23401).contains(&t)
}

// ---------- Own state ----------

/// State of the family (`rec` field of the mob).
#[derive(Debug, Clone)]
pub struct Rec {
    // Creaking.
    pub can_move: bool,
    pub active: bool,
    pub tearing_down: bool,
    pub home: Option<[i32; 3]>,
    pub attack_anim: i32,
    pub invul_anim: i32,
    pub stuck_counter: i32,
    // Breeze.
    pub sound_tick: i32,
    pub jump_trail: i32,
    // Warden.
    pub anger: warden::Anger,
    pub vib: vibration::VibData,
    pub client_anger: i32,
    // Happy ghast.
    pub still_timeout: i32,
    pub leash_time: i32,
    pub leash_holder: bool,
    pub stays_still: bool,
    pub float_duration: i32,
    pub restriction: Option<([i32; 3], i32)>,
    /// Setting (baby or adult) applied last.
    pub ghast_baby: Option<bool>,
}

/// World state specific to the family (`rec` field of the mobs).
#[derive(Debug, Clone, Default)]
pub struct RecWorld {
    pub hearts: heart::Hearts,
    pub vib: vibration::Vibrations,
    /// Vibration blocks (shriekers, sculk sensors): position → ticker id.
    pub vib_blocks: hashbrown::HashMap<[i32; 3], u64>,
    /// The player's warning tracking (`warden_spawn_tracker`).
    pub warden_tracker: shrieker::Tracker,
    /// Player steps (vibrations): distance traveled and next step.
    pub player_move_dist: f32,
    pub player_next_step: f32,
    /// Player in water at the last movement (splash on entry).
    pub player_in_water: bool,
    /// Explicit damage source in progress (`/damage … by X [from Y]`): true if the author is
    /// also the direct entity. `None`: deduced from the damage type.
    pub direct_source: Option<bool>,
}

impl Default for Rec {
    fn default() -> Rec {
        Rec {
            can_move: true,
            active: false,
            tearing_down: false,
            home: None,
            attack_anim: 0,
            invul_anim: 0,
            stuck_counter: 0,
            sound_tick: 0,
            jump_trail: 0,
            anger: warden::Anger::default(),
            vib: vibration::VibData::default(),
            client_anger: 0,
            still_timeout: 0,
            leash_time: 0,
            leash_holder: false,
            stays_still: false,
            float_duration: 0,
            restriction: None,
            ghast_baby: None,
        }
    }
}

/// Constructor: brain (sensor delays from the entity random), own attributes.
pub fn on_new(m: &mut Mob) {
    let b = match m.kind {
        Kind::Warden => warden::brain(&mut m.rng),
        Kind::Breeze => breeze::brain(&mut m.rng),
        Kind::Creaking => creaking::brain(&mut m.rng),
        _ => happy_ghast::brain(&mut m.rng),
    };
    m.bm.brain = Some(Box::new(b));
    match m.kind {
        Kind::Warden => {
            m.attrs.knockback_resistance.base = 1.0;
            m.attrs.attack_knockback.base = 1.5;
            warden::on_new(m);
        }
        Kind::HappyGhast => {
            m.attrs.tempt_range.base = 16.0;
        }
        _ => {}
    }
}

/// Navigation by species (server-side construction).
pub fn init(m: &mut Mob) {
    match m.kind {
        Kind::Warden | Kind::Creaking => m.nav.can_float = true,
        Kind::HappyGhast => happy_ghast::setup_nav(m),
        _ => {}
    }
}

/// "Classic" goals: only the adult happy ghast has any.
pub fn register_goals(m: &mut Mob) {
    if m.kind == Kind::HappyGhast {
        happy_ghast::register_goals(m);
    }
}

// ---------- Metadata ----------

pub fn define_data(m: &mut Mob) {
    match m.kind {
        // Warden's client-side anger level data.
        Kind::Warden => m.define(16, Meta::Int(0)),
        // Creaking: can move, active, tears down, home.
        Kind::Creaking => {
            m.define(16, Meta::Bool(true));
            m.define(17, Meta::Bool(false));
            m.define(18, Meta::Bool(false));
            m.define(19, Meta::BlockPos(None));
        }
        Kind::Breeze => {}
        // The "growing mob" layer (baby, frozen age), then leash holder and motionless.
        _ => {
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(false));
            m.define(18, Meta::Bool(false));
            m.define(19, Meta::Bool(false));
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    match m.kind {
        Kind::Warden => m.set(16, Meta::Int(m.rec.client_anger)),
        Kind::Creaking => {
            m.set(16, Meta::Bool(m.rec.can_move));
            m.set(17, Meta::Bool(m.rec.active));
            m.set(18, Meta::Bool(m.rec.tearing_down));
            m.set(19, Meta::BlockPos(m.rec.home));
        }
        Kind::Breeze => {}
        _ => {
            m.set(16, Meta::Bool(m.age < 0));
            m.set(17, Meta::Bool(m.mount.age_locked));
            m.set(18, Meta::Bool(m.rec.leash_holder));
            m.set(19, Meta::Bool(m.rec.stays_still));
        }
    }
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    match m.kind {
        Kind::Warden => warden::load(m, e),
        Kind::Creaking => {
            if let Some(Nbt::IntArray(v)) = e.get("home_pos")
                && v.len() == 3
            {
                creaking::set_transient(m, [v[0], v[1], v[2]]);
            }
        }
        Kind::HappyGhast => happy_ghast::load(m, e),
        _ => {}
    }
    if let Some(b) = m.bm.brain.as_mut() {
        super::brainy::nbt::load_memories(&mut b.mem, e.get("Brain"));
        // Same name as the sniffer's cooldown: the warden's lives in its own memories.
        if m.kind == Kind::Warden {
            b.mem.x.sniff_cooldown = b.mem.sniff_cd.clone();
            b.mem.sniff_cd.erase();
        }
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    match m.kind {
        Kind::Warden => warden::save(m, e),
        Kind::Creaking => {
            if let Some(h) = m.rec.home {
                crate::nbtio::set(e, "home_pos", Nbt::IntArray(vec![h[0], h[1], h[2]]));
            }
        }
        Kind::HappyGhast => happy_ghast::save(m, e),
        _ => {}
    }
    if let Some(b) = m.bm.brain.as_ref() {
        let mut mem = b.mem.clone();
        if m.kind == Kind::Warden {
            mem.sniff_cd = mem.x.sniff_cooldown.clone();
        }
        crate::nbtio::set(e, "Brain", super::brainy::nbt::save_memories(m.kind, &mem));
    }
}

// ---------- Navigation ----------

/// Linked heart of the creaking, for the path computation.
pub fn path_home(m: &Mob) -> Option<[i32; 3]> {
    (m.kind == Kind::Creaking).then_some(m.rec.home).flatten()
}

/// Path node evaluator around the home: cell blocked beyond 32 blocks from the heart, unless the mob is
/// itself at least as far.
pub fn home_blocks(home: [i32; 3], p: [i32; 3], mob: [i32; 3]) -> bool {
    let d = |a: [i32; 3]| (0..3).map(|i| f64::from(a[i] - home[i]).powi(2)).sum::<f64>();
    let hd = d(p);
    hd > 1024.0 && hd >= d(mob)
}

/// Own maluses (warden's constructor; linked creaking: the transient marking).
pub fn view_malus(k: Kind, bound: bool, t: PathType) -> Option<f32> {
    use PathType::*;
    match (k, t) {
        (Kind::Warden, UnpassableRail) => Some(0.0),
        (Kind::Warden, Damaging | PowderSnow | Lava) => Some(8.0),
        (Kind::Warden, Fire | FireInNeighbor) => Some(0.0),
        (Kind::Breeze, OnTopOfTrapdoor | Fire) => Some(-1.0),
        (Kind::Creaking, Damaging | PowderSnow | Lava) if bound => Some(8.0),
        (Kind::Creaking, Fire | FireInNeighbor) if bound => Some(0.0),
        _ => None,
    }
}

/// Walk value: warden and creaking 0; happy ghast according to the void.
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> Option<f32> {
    match m.kind {
        Kind::Warden | Kind::Creaking => Some(0.0),
        Kind::HappyGhast => Some(happy_ghast::walk_target_value(s, p)),
        _ => None,
    }
}

// ---------- Sounds ----------

/// Ambient sound; `None`: nothing.
pub fn ambient_sound(m: &Mob) -> Option<String> {
    match m.kind {
        Kind::Warden => warden::ambient_sound(m).map(Into::into),
        // Breeze: the sound played locally client-side only.
        Kind::Breeze => None,
        Kind::Creaking => (!m.rec.active).then(|| "minecraft:entity.creaking.ambient".into()),
        _ => Some(
            if m.is_baby() { "minecraft:entity.ghastling.ambient" } else { "minecraft:entity.happy_ghast.ambient" }
                .into(),
        ),
    }
}

/// Ambient sound interval: ridden happy ghast × 6.
pub fn ambient_interval(m: &Mob) -> Option<i32> {
    (m.kind == Kind::HappyGhast && !m.mount.passengers.is_empty()).then(|| m.kind.info().ambient_interval * 6)
}

/// Damage or death sound; `None`: info sheet.
pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    match m.kind {
        Kind::Creaking if !death && m.rec.home.is_some() => Some("minecraft:entity.creaking.sway"),
        Kind::HappyGhast => Some(match (m.is_baby(), death) {
            (true, false) => "minecraft:entity.ghastling.hurt",
            (true, true) => "minecraft:entity.ghastling.death",
            (false, false) => "minecraft:entity.happy_ghast.hurt",
            (false, true) => "minecraft:entity.happy_ghast.death",
        }),
        _ => None,
    }
}

/// Volume of the species' sounds; `None`: info sheet.
pub fn sound_volume(m: &Mob) -> Option<f32> {
    (m.kind == Kind::HappyGhast).then(|| if m.is_baby() { 1.0 } else { 4.0 })
}

/// Fixed voice pitch (happy ghast: 1).
pub fn fixed_voice_pitch(m: &Mob) -> Option<f32> {
    (m.kind == Kind::HappyGhast).then_some(1.0)
}

/// Sound category (happy ghast: neutral).
pub fn sound_source(m: &Mob) -> Option<i32> {
    (m.kind == Kind::HappyGhast).then_some(6)
}

/// Own step noise (sound, volume): warden at 10, creaking at 0.15; breeze and happy ghast
/// silent (void or emission through events only).
pub fn step_sound(m: &Mob) -> Option<(&'static str, f32)> {
    match m.kind {
        Kind::Warden => Some(("minecraft:entity.warden.step", 10.0)),
        Kind::Creaking => Some(("minecraft:entity.creaking.step", 0.15)),
        _ => None,
    }
}

/// No step sound at all (breeze: "events" emission; happy ghast: void).
pub fn silent_steps(k: Kind) -> bool {
    matches!(k, Kind::Breeze | Kind::HappyGhast)
}

/// Next step: warden every 0.55.
pub fn next_step(m: &Mob) -> Option<f32> {
    (m.kind == Kind::Warden).then_some(m.move_dist + 0.55)
}

// ---------- Spawning and despawning ----------

/// The warden never despawns far from players; the happy ghast is an animal.
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    match m.kind {
        Kind::Warden | Kind::HappyGhast => Some(false),
        _ => None,
    }
}

/// Maximum group size (happy ghast: 1).
pub fn max_cluster(k: Kind) -> Option<i32> {
    (k == Kind::HappyGhast).then_some(1)
}

/// Finalization before the common finalization.
pub fn finalize_pre(s: &mut Server, m: &mut Mob, reason: super::spawn::Reason) {
    if m.kind == Kind::Warden {
        warden::finalize(s, m, reason);
    }
}

// ---------- Tick hooks ----------

/// Start of the tick (`tick` before `super.tick`): creaking heart, warden vibrations, breeze random.
/// True if the mob must no longer tick.
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Creaking => creaking::pre_tick(s, m),
        Kind::Warden => warden::pre_tick(s, m),
        Kind::Breeze => breeze::pre_tick(s, m),
        _ => {}
    }
}

/// End of the common tick (`tick` after `super.tick`): happy ghast.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::HappyGhast {
        happy_ghast::post_tick(s, m);
    }
}

/// Death in progress: creaking that tears down (45 ticks then breaking apart). True if
/// the hook replaces the common countdown.
pub fn tick_death(s: &mut Server, m: &mut Mob) -> bool {
    m.kind == Kind::Creaking && creaking::tick_death(s, m)
}

/// Start of the AI step: creaking animations and motionlessness, precise position of the ghast.
pub fn pre_ai_step(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Creaking => creaking::pre_ai_step(s, m),
        Kind::HappyGhast => happy_ghast::pre_ai_step(m),
        _ => {}
    }
}

/// End of the AI step: continuous healing of the happy ghast.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::HappyGhast {
        happy_ghast::continuous_heal(s, m);
    }
}

/// Brain and own mechanics.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    if !handles(m.kind) {
        return;
    }
    let Some(mut b) = m.bm.brain.take() else { return };
    match m.kind {
        Kind::Warden => warden::custom_ai_step(s, m, &mut b),
        Kind::Breeze => {
            s.bz_brain_tick(m, &mut b);
            breeze::update_activity(&mut b);
        }
        Kind::Creaking => {
            s.bz_brain_tick(m, &mut b);
            creaking::update_activity(m, &mut b);
        }
        _ => happy_ghast::custom_ai_step(s, m, &mut b),
    }
    // Attack target of the brain.
    if m.kind != Kind::HappyGhast {
        m.target = b.mem.attack.get();
    }
    m.bm.brain = Some(b);
}

/// Frozen creaking: navigation, movement, gaze and jump controls, and body at rest.
pub fn frozen(m: &Mob) -> bool {
    m.kind == Kind::Creaking && !m.rec.can_move
}

/// Body rotation (before the common one); true if the common one is skipped. Frozen creaking: nothing;
/// ridden happy ghast: head and body set to the yaw, then common rotation.
pub fn body_rotation_pre(m: &mut Mob) -> bool {
    match m.kind {
        Kind::Creaking => !m.rec.can_move,
        Kind::HappyGhast => {
            if !m.mount.passengers.is_empty() {
                m.head_yaw = m.yaw;
                m.body_yaw = m.head_yaw;
            }
            false
        }
        _ => false,
    }
}

/// Precise position required by network tracking (immobilized happy ghast).
pub fn requires_precise_position(m: &Mob) -> bool {
    m.kind == Kind::HappyGhast && happy_ghast::on_still_timeout(m)
}

/// Own movement control; true if it replaces the common one.
pub fn move_control(s: &Server, m: &mut Mob) -> bool {
    match m.kind {
        Kind::Creaking if !m.rec.can_move => true,
        Kind::HappyGhast => happy_ghast::move_control(s, m),
        _ => false,
    }
}

/// Own gaze control; true if it replaces the common one.
pub fn look_control(s: &Server, m: &mut Mob) -> bool {
    match m.kind {
        Kind::Creaking => !m.rec.can_move,
        Kind::HappyGhast => happy_ghast::look_control(s, m),
        _ => false,
    }
}

/// Own movement (`travel`); true if it replaces the common one.
pub fn travel(s: &mut Server, m: &mut Mob) -> bool {
    if m.kind == Kind::HappyGhast {
        happy_ghast::travel(s, m);
        return true;
    }
    false
}

// ---------- Combat ----------

/// Before damage (overridden); `Some`: imposed result.
pub fn before_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) -> Option<bool> {
    match m.kind {
        Kind::Creaking => creaking::before_hurt(s, m, ty, by),
        Kind::Warden => warden::before_hurt(s, m, ty),
        Kind::Breeze => {
            // Immune to damage caused by a breeze.
            let by_breeze = matches!(by, Some(Ent::Mob(id)) if s.mob_ref(id).is_some_and(|o| o.kind == Kind::Breeze));
            by_breeze.then_some(false)
        }
        _ => None,
    }
}

/// After damage taken: warden anger.
pub fn after_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) {
    if m.kind == Kind::Warden {
        warden::after_hurt(s, m, ty, by);
    }
}

/// Melee hit dealt, before the common hit.
pub fn before_melee(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Warden => warden::before_melee(s, m),
        Kind::Creaking => {
            m.rec.attack_anim = 15;
            s.mob_event(m, 4);
        }
        _ => {}
    }
}

/// Knockback accepted (`knockback` overridden: frozen creaking motionless).
pub fn accepts_knockback(m: &Mob) -> bool {
    !frozen(m)
}

/// Pushed by other entities.
pub fn pushable(m: &Mob) -> bool {
    match m.kind {
        Kind::Creaking => m.rec.can_move,
        Kind::Warden => !warden::digging_or_emerging(m),
        _ => true,
    }
}

/// Explosions ignored (warden digging or emerging).
pub fn ignores_explosion(m: &Mob) -> bool {
    m.kind == Kind::Warden && warden::digging_or_emerging(m)
}

/// Fire-immune (creaking linked to its heart, warden).
pub fn fire_immune(m: &Mob) -> bool {
    m.kind == Kind::Creaking && m.rec.home.is_some()
}

impl Server {
    /// Right click on a mob of the family; `None`: common handling.
    pub(crate) fn recent_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        (m.kind == Kind::HappyGhast).then(|| happy_ghast::interact(self, m, slot, item)).flatten()
    }
}

/// Spawn data of the entity (warden emerging: 1).
pub fn add_entity_data(m: &Mob) -> i32 {
    if m.kind == Kind::Warden { warden::add_entity_data(m) } else { 0 }
}

impl Server {
    // ---------- Common tools ----------

    /// Block sound at the center of the block.
    pub(crate) fn block_sound(&mut self, p: [i32; 3], name: &str, volume: f32, pitch: f32) {
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.world_sound(name, 4, c, volume, pitch);
    }

    /// Block state particles.
    pub(crate) fn state_particles(
        &self,
        name: &str,
        state: u32,
        pos: [f64; 3],
        delta: [f32; 3],
        speed: f32,
        count: i32,
    ) {
        let Some(kind) =
            self.data.registry("minecraft:particle_type").and_then(|r| r.entries.iter().position(|e| e == name))
        else {
            return;
        };
        let p = rc_proto_v777::entity::LevelParticles {
            particle: rc_proto_v777::entity::Particle {
                kind: kind as i32,
                color: 0,
                state: state as i32,
                item: -1,
                scale: 1.0,
                color2: 0,
                vib: None,
            },
            force: false,
            always_visible: false,
            pos,
            delta,
            speed: [speed; 3],
            count,
            mode: 0,
        };
        let _ = self.send(rc_client::Event::Particles(p));
    }

    /// Player push (`push`): velocity added, sent back to the client.
    pub(crate) fn player_push(&mut self, d: [f64; 3]) {
        let v = self.mobs.player_vel;
        let nv = [v[0] + d[0], v[1] + d[1], v[2] + d[2]];
        self.mobs.player_vel = nv;
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Motion {
            id: self.player_id,
            velocity: super::track::lp_quantize(nv),
        }));
    }

    /// Is the entity a passenger of a vehicle?
    pub(crate) fn ent_riding(&self, e: Ent) -> bool {
        match e {
            Ent::Player => self.player_riding_any(),
            Ent::Mob(id) => self.mob_ref(id).is_some_and(|m| m.mount.vehicle.is_some()),
        }
    }

    /// Non-zero precipitation at the location: rain, visible sky, above the heightmap, biome
    /// with precipitation.
    pub(crate) fn precipitation_at(&self, p: [i32; 3]) -> bool {
        if !self.is_raining_at(p) {
            return false;
        }
        let biome = self.biome_str(p);
        self.life
            .pack
            .get("worldgen/biome", biome.trim_start_matches("minecraft:"))
            .is_none_or(|b| b["has_precipitation"].as_bool().unwrap_or(true))
    }

    /// Fluid height of a cell (1 under the same fluid).
    pub(crate) fn fluid_height_at(&self, p: [i32; 3]) -> f64 {
        let f = self.fluid_at(p);
        if f.is_empty() {
            return 0.0;
        }
        if self.fluid_at([p[0], p[1] + 1, p[2]]).kind == f.kind { 1.0 } else { f64::from(f.own_height()) }
    }

    /// Tries at ±`xz` (height `y`), descent to a full support
    /// under a collision-free cell (without leaves if requested), free box if requested;
    /// finalized mob, rule (walk value ≥ 0) and obstruction, then ambient sound.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_util(
        &mut self,
        kind: Kind,
        reason: super::spawn::Reason,
        start: [i32; 3],
        attempts: i32,
        xz: i32,
        y: i32,
        check_collisions: bool,
        no_leaves: bool,
    ) -> Option<i32> {
        for _ in 0..attempts {
            let dx = self.mobs.rng.next_int_bound(xz * 2 + 1) - xz;
            let dz = self.mobs.rng.next_int_bound(xz * 2 + 1) - xz;
            let mut sp = [start[0] + dx, start[1] + y, start[2] + dz];
            if !self.spawn_util_ground(y, &mut sp, no_leaves) {
                continue;
            }
            let i = kind.info();
            let pos = [f64::from(sp[0]) + 0.5, f64::from(sp[1]), f64::from(sp[2]) + 0.5];
            if check_collisions {
                let (w, h) = (f64::from(i.width * i.spawn_scale), f64::from(i.height * i.spawn_scale));
                let b = rc_physics::Aabb {
                    min: [pos[0] - w / 2.0, pos[1], pos[2] - w / 2.0],
                    max: [pos[0] + w / 2.0, pos[1] + h, pos[2] + w / 2.0],
                };
                if !rc_physics::is_free(&self.world(), &b) {
                    continue;
                }
            }
            // Yaw drawn on the world random, finalization.
            let yaw = super::mth::wrap(self.mobs.rng.next_float() * 360.0);
            let mut m = self.new_mob(kind, pos, yaw);
            m.head_yaw = m.yaw;
            m.body_yaw = m.yaw;
            let mut g = super::spawn::Group::default();
            self.finalize_spawn(&mut m, reason, &mut g);
            if self.walk_target_value(&m, sp) >= 0.0 && self.spawn_util_unobstructed(&m) {
                let id = m.id;
                if let Some(snd) = ambient_sound(&m) {
                    let v = m.sound_volume();
                    let p = Server::voice_pitch(&mut m);
                    self.mob_sound(&m, &snd, v, p);
                }
                self.add_mob(m);
                return Some(id);
            }
        }
        None
    }

    /// The movement toward a possible spawn position ("on a support" strategies, with or without leaves).
    fn spawn_util_ground(&self, range: i32, sp: &mut [i32; 3], no_leaves: bool) -> bool {
        let empty_shape = |s: &Server, p: [i32; 3]| {
            let mut b = Vec::new();
            s.phys.collision(s.st(p), &mut b);
            b.is_empty()
        };
        let mut above_empty = empty_shape(self, *sp);
        for _ in (-range..=range).rev() {
            sp[1] -= 1;
            let cur = self.st(*sp);
            let ok = above_empty && (!no_leaves || !self.btag_id(*sp, crate::tag_bits::TagId::Leaves)) && self.face_full_up(cur);
            if ok {
                sp[1] += 1;
                return true;
            }
            above_empty = empty_shape(self, *sp);
        }
        false
    }

    /// Neither liquid in the box, nor an entity intersecting it (warden:
    /// nor a block).
    fn spawn_util_unobstructed(&self, m: &Mob) -> bool {
        let bb = m.bbox();
        if self.box_has_liquid(&bb) {
            return false;
        }
        if !self.unobstructed(&bb, m.id) {
            return false;
        }
        if m.kind == Kind::Warden && !rc_physics::is_free(&self.world(), &bb) {
            return false;
        }
        true
    }

    // ---------- Related blocks (block engine hooks) ----------

    /// Scheduled tick of a block of the family; true if handled.
    pub(crate) fn rec_block_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        self.heart_block_tick(state, p)
            || self.shrieker_block_tick(state, p)
            || self.sensor_block_tick(state, p)
            || self.dried_ghast_tick(state, p)
    }

    /// Random tick of a block of the family; true if handled.
    pub(crate) fn rec_random_tick(&mut self, state: u32, p: [i32; 3]) -> bool {
        self.dried_ghast_random_tick(state, p)
    }

    /// The heart's shape update: tick scheduled at the next tick.
    pub(crate) fn rec_update_shape(&mut self, state: u32, p: [i32; 3]) {
        if heart::is_heart_block(self.state_name(state)) {
            let b = self.v().block(state);
            self.schedule_tick(p, b, 1);
        }
    }

    /// Block entity removed.
    pub(crate) fn rec_pre_remove(&mut self, p: [i32; 3], old: u32) {
        let name = self.state_name(old).to_owned();
        if heart::is_heart_block(&name) {
            self.heart_pre_remove(p);
        } else if shrieker::is_shrieker(&name) {
            self.shrieker_pre_remove(p, old);
        } else if vibration::block_ear(&name).is_some() {
            self.vib_block_forget(p);
        }
    }

    /// The player is about to break a block.
    pub(crate) fn rec_player_will_destroy(&mut self, p: [i32; 3], state: u32) {
        self.heart_player_will_destroy(p, state);
        // "Block destroyed" vibration attributed to the player.
        self.game_event_block("minecraft:block_destroy", p, Some(Ent::Player), Some(state));
    }

    /// The player moved (`move` server-side): step vibrations (the trampled shrieker goes through block effects, every tick).
    pub(crate) fn rec_player_moved(&mut self, before: [f64; 3]) {
        self.update_player_water();
        if self.mobs.rec.vib.listeners.is_empty() && self.mobs.rec.vib_blocks.is_empty() {
            return;
        }
        // Zero movement emission (flight, or sneaking on the ground): neither step distance accumulation,
        // nor step event.
        if self.flying || (self.on_ground && self.sneaking) {
            return;
        }
        let d = [self.pos[0] - before[0], self.pos[1] - before[1], self.pos[2] - before[2]];
        let h = (d[0] * d[0] + d[2] * d[2]).sqrt();
        // Supporting block (gating and vibration state) and block at 0.2 (effects, shrieker).
        let (on, effect) = self.player_on_pos();
        let on_st = self.st(on);
        let es = self.st(effect);
        let climbing = self.climbable_state(on_st);
        let full = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let r = &mut self.mobs.rec;
        r.player_move_dist += if climbing { (full * f64::from(0.6f32)) as f32 } else { (h * f64::from(0.6f32)) as f32 };
        if self.mobs.rec.player_move_dist > self.mobs.rec.player_next_step && !self.v().is_air(on_st) {
            // The step on the block at 0.2 vibrates only if it is also the supporting block.
            let same = on == effect;
            let mut produced = self.player_step_effects(es, same, d[1]);
            if !same {
                produced |= self.player_step_effects(on_st, true, d[1]);
            }
            if produced {
                self.mobs.rec.player_next_step = self.mobs.rec.player_move_dist.trunc() + 1.0;
            } else if self.fluid_at(self.pos.map(|v| v.floor() as i32)).kind == 1 {
                self.mobs.rec.player_next_step = self.mobs.rec.player_move_dist.trunc() + 1.0;
                let pos = self.pos;
                self.game_event_src("minecraft:swim", pos, Some(vibration::Src::Living(Ent::Player)), None);
            }
        }
    }

    /// Player step on a block: nothing on air; on the ground, climbing or sneaking without
    /// vertical movement, and not swimming: possible vibration (the sound comes from the client).
    fn player_step_effects(&mut self, st: u32, vibrate: bool, dy: f64) -> bool {
        if self.v().is_air(st) {
            return false;
        }
        let climb = self.climbable_state(st);
        let grounded = self.on_ground || climb || (self.sneaking && dy == 0.0);
        if !grounded || self.mobs.sea.player_swimming {
            return false;
        }
        if vibrate {
            let pos = self.pos;
            self.game_event_src("minecraft:step", pos, Some(vibration::Src::Living(Ent::Player)), Some(st));
        }
        true
    }

    /// Main supporting block of the player on the ground (the closest among those that support them).
    pub(crate) fn player_support(&self) -> Option<[i32; 3]> {
        if !self.on_ground {
            return None;
        }
        let p = self.pos;
        let b = rc_physics::Aabb { min: [p[0] - 0.3, p[1], p[2] - 0.3], max: [p[0] + 0.3, p[1] + 1.8, p[2] + 0.3] };
        let w = self.world();
        rc_physics::support_of(&w, &b, p)
    }

    /// Block under the feet and effect block at 0.2: the supporting
    /// block serves for both, except that at 0.2 we keep its column and the feet height −0.2
    /// (fences, walls and fence gates keep the supporting block).
    pub(crate) fn player_on_pos(&self) -> ([i32; 3], [i32; 3]) {
        let s = self.player_support();
        (self.on_pos(self.pos, s, 1.0e-5), self.on_pos(self.pos, s, 0.2))
    }

    /// Water of the feet on movement (swimming itself comes from `sealife_player_tick`, once
    /// per tick).
    fn update_player_water(&mut self) {
        let feet = self.pos.map(|v| v.floor() as i32);
        let in_water = self.fluid_at(feet).kind == 1;
        // Entry into water (not as spectator): `splash` event;
        // the sound goes to the other players only.
        let was = std::mem::replace(&mut self.mobs.rec.player_in_water, in_water);
        if in_water && !was && self.game_mode != 3 {
            let pos = self.pos;
            self.game_event_at("minecraft:splash", pos, Some(Ent::Player));
        }
    }

    /// Block entity created: hearts, shriekers and sculk sensors registered.
    pub(crate) fn rec_block_entity_created(&mut self, p: [i32; 3], state: u32) {
        let name = self.state_name(state).to_owned();
        if heart::is_heart_block(&name) {
            self.heart_register(p);
        } else if vibration::block_ear(&name).is_some() {
            self.vib_block_register(p);
        }
    }

    /// Chunk loaded: hearts, shriekers and sensors of its block entities registered.
    pub(crate) fn rec_register_chunk(&mut self, ch: (i32, i32)) {
        let Some(Some(c)) = self.chunks.get(&ch) else { return };
        let found: Vec<(bool, [i32; 3])> = c
            .block_entities
            .iter()
            .filter_map(|b| {
                let id = b.get("id").and_then(Nbt::as_str)?;
                let heart = id == "minecraft:creaking_heart";
                if !heart && vibration::block_ear(id).is_none() {
                    return None;
                }
                let i = |k: &str| crate::nbtio::int(b, k).unwrap_or(0) as i32;
                Some((heart, [i("x"), i("y"), i("z")]))
            })
            .collect();
        for (heart, p) in found {
            if heart {
                self.heart_register(p);
            } else {
                self.vib_block_register(p);
            }
        }
    }

    /// Family ticker: alive? (`None`: not theirs.)
    pub(crate) fn rec_ticker_alive(&mut self, id: u64, p: [i32; 3]) -> Option<bool> {
        if self.heart_is_ticker(id, p) {
            return Some(self.heart_ticker_alive(id, p));
        }
        if self.vib_block_is_ticker(id, p) {
            return Some(self.vib_block_ticker_alive(id, p));
        }
        None
    }

    /// Tick of a family ticker; true if it belongs to it.
    pub(crate) fn rec_tick_ticker(&mut self, id: u64, p: [i32; 3]) -> bool {
        if self.heart_is_ticker(id, p) {
            self.heart_tick(p);
            return true;
        }
        if self.vib_block_is_ticker(id, p) {
            self.vib_tick_block(p);
            return true;
        }
        false
    }

    /// World tick specific to the family: player warnings (before the mobs).
    pub(crate) fn rec_world_tick(&mut self) {
        self.mobs.rec.warden_tracker.tick();
        // G6-F10: player on the ground on a sculk sensor or a shrieker (block effects, every tick).
        if self.on_ground && self.game_mode != 3 && !self.mobs.rec.vib_blocks.is_empty() {
            let support = self.player_support();
            let pos = self.pos;
            self.sensor_entity_on(pos, support, vibration::Src::Living(Ent::Player), false);
        }
    }
}
