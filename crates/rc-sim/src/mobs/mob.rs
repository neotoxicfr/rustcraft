//! Simulated mob: common state (entity, living entity, mob), attributes, synced
//! metadata, NBT reading and writing (`entities/`).

use rc_physics::Aabb;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::goals::Selector;
use super::jrand::JRand;
use super::nav::Nav;
use super::species::{Kind, dims};
use crate::nbtio::{self, int};
use crate::player::Stack;

/// Switch read once: absent or different from "0" → true.
fn opt_switch(cell: &'static std::sync::OnceLock<bool>, name: &str) -> bool {
    *cell.get_or_init(|| rc_data::env_flags::read_os(name).and_then(|v| v.into_string().ok()).is_none_or(|v| v != "0"))
}

/// `RUSTCRAFT_OPT_DIMSONCE=0`: box through `width()` then `height()` (old path).
pub(crate) fn dims_once() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    opt_switch(&ON, "RUSTCRAFT_OPT_DIMSONCE")
}

/// `RUSTCRAFT_OPT_PUSHFILTER=0`: no box prefilter (old path).
pub(crate) fn push_filter() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    opt_switch(&ON, "RUSTCRAFT_OPT_PUSHFILTER")
}

/// Living-entity reference: the player, or a mob by network id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ent {
    Player,
    Mob(i32),
}

/// Attribute: base and modifiers (id, amount, operation 0/1/2).
#[derive(Debug, Clone)]
pub struct Attr {
    pub base: f64,
    pub mods: Vec<(&'static str, f64, i32)>,
    /// Permanent modifiers (saved): spawn bonus, `/attribute`, read back from the NBT;
    /// the others (effects, equipment, transient states) are not saved.
    pub perm: Vec<&'static str>,
    /// Base set by the data or a command: the attribute is rewritten on save.
    pub saved: bool,
    /// Bounds of the attribute: the computed value is brought back into them, an undefined result (NaN)
    /// gives the minimum (a strong Slowness stops the mob instead of making it move backwards).
    pub lo: f64,
    pub hi: f64,
}

impl Default for Attr {
    fn default() -> Attr {
        Attr::new(0.0)
    }
}

impl Attr {
    /// Attribute without bounds (internal values, trials).
    pub fn new(base: f64) -> Attr {
        Attr::ranged(base, f64::NEG_INFINITY, f64::INFINITY)
    }

    /// Attribute bounded to `[lo, hi]` (bounds of the attribute registry).
    pub fn ranged(base: f64, lo: f64, hi: f64) -> Attr {
        Attr { base, mods: Vec::new(), perm: Vec::new(), saved: false, lo, hi }
    }

    /// Permanent modifier: kept on save.
    pub fn set_perm_mod(&mut self, id: &'static str, amount: f64, op: i32) {
        self.set_mod(id, amount, op);
        if !self.perm.contains(&id) {
            self.perm.push(id);
        }
    }

    pub fn value(&self) -> f64 {
        let mut b = self.base;
        for m in self.mods.iter().filter(|m| m.2 == 0) {
            b += m.1;
        }
        let mut r = b;
        for m in self.mods.iter().filter(|m| m.2 == 1) {
            r += b * m.1;
        }
        for m in self.mods.iter().filter(|m| m.2 == 2) {
            r *= 1.0 + m.1;
        }
        if r.is_nan() && self.lo.is_finite() { self.lo } else { r.clamp(self.lo, self.hi) }
    }

    pub fn set_mod(&mut self, id: &'static str, amount: f64, op: i32) {
        self.mods.retain(|m| m.0 != id);
        self.mods.push((id, amount, op));
    }

    pub fn remove_mod(&mut self, id: &'static str) {
        self.mods.retain(|m| m.0 != id);
        self.perm.retain(|p| *p != id);
    }
}

/// Ids (attributes, modifiers) kept for the lifetime of the process.
pub fn intern(s: &str) -> &'static str {
    use hashbrown::HashSet;
    use std::sync::Mutex;
    static SET: std::sync::OnceLock<Mutex<HashSet<&'static str>>> = std::sync::OnceLock::new();
    let m = SET.get_or_init(|| Mutex::new(HashSet::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(x) = g.get(s) {
        return x;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    g.insert(leaked);
    leaked
}

/// Operation of a modifier (registered name).
fn op_of(name: Option<&str>) -> i32 {
    match name {
        Some("add_multiplied_base") => 1,
        Some("add_multiplied_total") => 2,
        _ => 0,
    }
}

fn op_name(op: i32) -> &'static str {
    match op {
        1 => "add_multiplied_base",
        2 => "add_multiplied_total",
        _ => "add_value",
    }
}

#[derive(Debug, Clone)]
pub struct Attrs {
    pub max_health: Attr,
    pub speed: Attr,
    pub attack: Attr,
    pub follow: Attr,
    pub armor: Attr,
    pub step: Attr,
    pub knockback_resistance: Attr,
    pub attack_knockback: Attr,
    pub gravity: Attr,
    pub jump: Attr,
    pub safe_fall: Attr,
    pub fall_mult: Attr,
    pub tempt_range: Attr,
    /// F03: bounce, friction on the ground and in the air (modifiers), explosion knockback.
    pub bounciness: Attr,
    pub friction_mod: Attr,
    pub air_drag_mod: Attr,
    pub explosion_kb_resistance: Attr,
    /// Chance to call reinforcements (zombie family only: the others do not have
    /// the attribute, like in the game).
    pub reinforcements: Option<Attr>,
    /// Registered attributes that the simulation does not model (scale, sturdiness…): read back
    /// and rewritten as they are.
    pub extra: Vec<Nbt>,
}

impl Attrs {
    /// Modeled attributes, by id.
    pub const NAMES: [&'static str; 18] = [
        "minecraft:max_health",
        "minecraft:movement_speed",
        "minecraft:attack_damage",
        "minecraft:follow_range",
        "minecraft:armor",
        "minecraft:step_height",
        "minecraft:knockback_resistance",
        "minecraft:attack_knockback",
        "minecraft:gravity",
        "minecraft:jump_strength",
        "minecraft:safe_fall_distance",
        "minecraft:fall_damage_multiplier",
        "minecraft:tempt_range",
        "minecraft:bounciness",
        "minecraft:friction_modifier",
        "minecraft:air_drag_modifier",
        "minecraft:explosion_knockback_resistance",
        "minecraft:spawn_reinforcements",
    ];

    pub fn by_name(&self, n: &str) -> Option<&Attr> {
        Some(match n {
            "minecraft:max_health" => &self.max_health,
            "minecraft:movement_speed" => &self.speed,
            "minecraft:attack_damage" => &self.attack,
            "minecraft:follow_range" => &self.follow,
            "minecraft:armor" => &self.armor,
            "minecraft:step_height" => &self.step,
            "minecraft:knockback_resistance" => &self.knockback_resistance,
            "minecraft:attack_knockback" => &self.attack_knockback,
            "minecraft:gravity" => &self.gravity,
            "minecraft:jump_strength" => &self.jump,
            "minecraft:safe_fall_distance" => &self.safe_fall,
            "minecraft:fall_damage_multiplier" => &self.fall_mult,
            "minecraft:tempt_range" => &self.tempt_range,
            "minecraft:bounciness" => &self.bounciness,
            "minecraft:friction_modifier" => &self.friction_mod,
            "minecraft:air_drag_modifier" => &self.air_drag_mod,
            "minecraft:explosion_knockback_resistance" => &self.explosion_kb_resistance,
            "minecraft:spawn_reinforcements" => self.reinforcements.as_ref()?,
            _ => return None,
        })
    }

    pub fn by_name_mut(&mut self, n: &str) -> Option<&mut Attr> {
        Some(match n {
            "minecraft:max_health" => &mut self.max_health,
            "minecraft:movement_speed" => &mut self.speed,
            "minecraft:attack_damage" => &mut self.attack,
            "minecraft:follow_range" => &mut self.follow,
            "minecraft:armor" => &mut self.armor,
            "minecraft:step_height" => &mut self.step,
            "minecraft:knockback_resistance" => &mut self.knockback_resistance,
            "minecraft:attack_knockback" => &mut self.attack_knockback,
            "minecraft:gravity" => &mut self.gravity,
            "minecraft:jump_strength" => &mut self.jump,
            "minecraft:safe_fall_distance" => &mut self.safe_fall,
            "minecraft:fall_damage_multiplier" => &mut self.fall_mult,
            "minecraft:tempt_range" => &mut self.tempt_range,
            "minecraft:bounciness" => &mut self.bounciness,
            "minecraft:friction_modifier" => &mut self.friction_mod,
            "minecraft:air_drag_modifier" => &mut self.air_drag_mod,
            "minecraft:explosion_knockback_resistance" => &mut self.explosion_kb_resistance,
            "minecraft:spawn_reinforcements" => self.reinforcements.as_mut()?,
            _ => return None,
        })
    }

    /// Rereading of the `attributes` list: base and modifiers (all permanent); the
    /// unmodeled attributes are kept as they are.
    pub fn load(&mut self, list: &[Nbt]) {
        self.extra.clear();
        for a in list {
            let id = a.get("id").and_then(Nbt::as_str).unwrap_or("");
            let id = if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") };
            let Some(slot) = self.by_name_mut(&id) else {
                self.extra.push(a.clone());
                continue;
            };
            if let Some(b) = a.get("base").and_then(f64_of) {
                slot.base = b;
            }
            slot.saved = true;
            for md in nbtio::list(a.get("modifiers")) {
                let Some(mid) = md.get("id").and_then(Nbt::as_str) else { continue };
                let amount = md.get("amount").and_then(f64_of).unwrap_or(0.0);
                let op = op_of(md.get("operation").and_then(Nbt::as_str));
                slot.set_perm_mod(intern(mid), amount, op);
            }
        }
    }

    /// Registered `attributes` list: max HP, speed and follow range always, the others
    /// when their base comes from the data or they carry a permanent modifier.
    pub fn save(&self) -> Vec<Nbt> {
        let mut out = Vec::new();
        for (k, n) in Self::NAMES.iter().enumerate() {
            let Some(a) = self.by_name(n) else { continue };
            if k != 0 && k != 1 && k != 3 && !a.saved && a.perm.is_empty() {
                continue;
            }
            let mut entry = vec![("id".into(), Nbt::String((*n).into())), ("base".into(), Nbt::Double(a.base))];
            let mods: Vec<Nbt> = a
                .mods
                .iter()
                .filter(|m| a.perm.contains(&m.0))
                .map(|(id, v, op)| {
                    Nbt::Compound(vec![
                        ("id".into(), Nbt::String((*id).into())),
                        ("amount".into(), Nbt::Double(*v)),
                        ("operation".into(), Nbt::String(op_name(*op).into())),
                    ])
                })
                .collect();
            if !mods.is_empty() {
                entry.push(("modifiers".into(), Nbt::List(mods)));
            }
            out.push(Nbt::Compound(entry));
        }
        out.extend(self.extra.iter().cloned());
        out
    }
}

/// Movement control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoveOp {
    Wait,
    MoveTo,
    Strafe,
    Jumping,
}

#[derive(Debug, Clone)]
pub struct MoveCtl {
    pub op: MoveOp,
    pub wanted: [f64; 3],
    pub speed_mod: f64,
    pub strafe_fwd: f32,
    pub strafe_right: f32,
}

#[derive(Debug, Clone, Default)]
pub struct LookCtl {
    pub y_max: f32,
    pub x_max: f32,
    pub cooldown: i32,
    pub wanted: [f64; 3],
}

#[derive(Debug, Clone, Default)]
pub struct BodyCtl {
    pub head_stable_time: i32,
    pub last_stable_head: f32,
}

/// Synced value: index, value, to send.
#[derive(Debug, Clone)]
pub struct Synced {
    pub index: u8,
    pub value: Meta,
    pub default: Meta,
    pub dirty: bool,
}

/// Last damage source (type, author, source position, tick).
#[derive(Debug, Clone)]
pub struct Hit {
    pub ty: String,
    pub by: Option<Ent>,
    pub tick: u64,
}

#[derive(Debug, Clone)]
pub struct Mob {
    pub id: i32,
    pub uuid: u128,
    pub kind: Kind,
    pub rng: JRand,
    // Entity.
    pub pos: [f64; 3],
    pub old: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    /// Head pitch and yaw at the start of the tick ("previous" values read by the AI).
    pub pitch_o: f32,
    pub head_yaw_o: f32,
    pub body_yaw: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub fall_distance: f64,
    pub tick_count: i32,
    pub fire: i32,
    pub air: i32,
    /// Freeze ticks (powder snow), 140 = frozen.
    pub ticks_frozen: i32,
    pub water_height: f64,
    pub lava_height: f64,
    pub in_water: bool,
    pub eyes_in_water: bool,
    pub was_eyes_in_water: bool,
    pub support: Option<[i32; 3]>,
    /// Piston pushes accumulated during the tick (cap of 0.51 per axis).
    pub piston_deltas: rc_physics::piston::PistonDeltas,
    pub move_dist: f32,
    pub next_step: f32,
    pub needs_sync: bool,
    pub removed: bool,
    // Living entity.
    pub attrs: Attrs,
    pub health: f32,
    pub hurt_time: i32,
    pub damage_cooldown: i32,
    pub last_hurt: f32,
    pub death_time: i32,
    pub dead: bool,
    pub no_jump_delay: i32,
    pub jumping: bool,
    pub xxa: f32,
    pub yya: f32,
    pub zza: f32,
    pub speed: f32,
    pub last_hurt_by_mob: Option<(Ent, i32)>,
    /// Last victim of a melee hit or an arrow (alive at the time of the test).
    pub last_hurt_mob: Option<Ent>,
    pub last_hurt_by_player: i32,
    pub last_hit: Option<Hit>,
    pub swing_time: i32,
    /// Item in use (drawn bow): ticks, −1 otherwise.
    pub use_ticks: i32,
    // Mob.
    pub no_action_time: i32,
    /// Zombie family: ability to break doors down (`CanBreakDoors`), distinct from
    /// opening doors through navigation.
    pub can_break_doors: bool,
    pub ambient_time: i32,
    pub persistent: bool,
    pub can_pickup: bool,
    pub left_handed: bool,
    /// Imposed death loot table (`DeathLootTable`, forgotten after the draw) and its seed
    /// (`DeathLootTableSeed`, 0 = none).
    pub death_loot: Option<String>,
    pub death_loot_seed: i64,
    pub no_ai: bool,
    /// `Invulnerable` flag of the NBT (only `bypasses_invulnerability` and a creative player get through).
    pub invulnerable: bool,
    pub aggressive: bool,
    pub target: Option<Ent>,
    pub move_ctl: MoveCtl,
    pub look: LookCtl,
    pub body: BodyCtl,
    pub jump_ctl: bool,
    pub nav: Nav,
    pub goals: Selector,
    pub targets: Selector,
    /// Lines of sight already computed this tick (target, view).
    pub sight: Vec<(Ent, bool)>,
    pub equipment: [Option<Stack>; 8],
    pub drop_chance: [f32; 8],
    // Age (animals, villagers) and baby zombie.
    pub age: i32,
    pub baby: bool,
    pub in_love: i32,
    pub love_cause: bool,
    // Particularities.
    pub swell: i32,
    pub old_swell: i32,
    pub swell_dir: i32,
    pub fuse: i32,
    pub explosion_radius: i32,
    pub powered: bool,
    pub ignited: bool,
    pub wool: u8,
    pub sheared: bool,
    pub eat_anim: i32,
    pub egg_time: i32,
    /// Chicken ridden by a baby zombie (`IsChickenJockey`): does not lay, despawns when far away.
    pub chicken_jockey: bool,
    pub carried: Option<u32>,
    pub creepy: bool,
    pub stared_at: bool,
    pub target_change_time: i32,
    pub anger_end: i64,
    pub anger_target: Option<Ent>,
    pub tame: bool,
    pub sitting: bool,
    /// Cat: sitting on a block (pose only, without a sit order), lying down, curled against
    /// the sleeping owner (first time).
    pub cat_sit_pose: bool,
    pub cat_lying: bool,
    pub cat_relax: bool,
    pub owner: Option<u128>,
    pub collar: i32,
    pub variant: i32,
    pub sound_variant: i32,
    pub villager: [i32; 3],
    pub climbing: bool,
    /// State specific to the F01 family (conversions, phantom, pests, effects).
    pub x: super::variants::Extra,
    /// F03: state specific to cubes (size, jumps, absorbed item…).
    pub cube: super::cubes::CubeState,
    // Network.
    pub data: Vec<Synced>,
    pub track: super::track::Tracker,
    /// Original NBT (unknown fields copied back on save).
    pub nbt: Option<Nbt>,
    pub custom_name: Option<Nbt>,
    /// State of simple surface animals (F02).
    pub surf: super::surface::Surf,
    /// F04: mounts and passengers (`mounts/`).
    pub mount: super::mounts::Mount,
    /// F05: merchant state (villager, wandering trader).
    pub npc: Option<Box<super::villager::Npc>>,
    /// F06: golem state (iron, snow, copper).
    pub golem: super::golems::Golem,
    /// F07: illagers, vex, ravager, witch (patrol, raid, spells…).
    pub ill: super::illagers::Ill,
    /// F08: drowned and guardians (`aquatic/`).
    pub aqua: super::aquatic::Aqua,
    /// F09: state of aquatic fauna and nautiluses (`sealife/`).
    pub sea: super::sealife::Aqua,
    /// F10: brain animals (`brainy/`).
    pub bm: super::brainy::Bm,
    /// F11: bee, fox, panda (`wild/`).
    pub wild: super::wild::Wild,
    /// F12: blaze, ghast, strider (`nether/`).
    pub neth: super::nether::Neth,
    /// F13: piglins and hoglins (`piglins/`).
    pub pig: super::piglins::Pig,
    /// F14: warden, breeze, creaking, happy ghast (`recent/`).
    pub rec: super::recent::Rec,
    /// F15: Ender dragon, shulker (`end/`).
    pub end: super::end::EndMob,
    /// F16: wither (`wither/`).
    pub wither: super::wither::Wither,
}

impl Mob {
    pub fn new(kind: Kind, id: i32, uuid: u128, pos: [f64; 3], yaw: f32, rng: JRand) -> Mob {
        let i = kind.info();
        let attrs = Attrs {
            // Bounds of the official attribute registry.
            max_health: Attr::ranged(i.health, 1.0, 1024.0),
            speed: Attr::ranged(i.speed, 0.0, 1024.0),
            attack: Attr::ranged(i.attack, 0.0, 2048.0),
            follow: Attr::ranged(i.follow, 0.0, 2048.0),
            armor: Attr::ranged(i.armor, 0.0, 30.0),
            step: Attr::ranged(i.step, 0.0, 10.0),
            knockback_resistance: Attr::ranged(0.0, -2.0, 1.0),
            attack_knockback: Attr::ranged(0.0, 0.0, 5.0),
            gravity: Attr::ranged(0.08, -1.0, 1.0),
            // Single-precision default (0.42F) widened to double, like the registry.
            jump: Attr::ranged(f64::from(0.42f32), 0.0, 32.0),
            safe_fall: Attr::ranged(3.0, -1024.0, 1024.0),
            fall_mult: Attr::ranged(1.0, 0.0, 100.0),
            tempt_range: Attr::ranged(if kind == Kind::SulfurCube { 8.0 } else { 10.0 }, 0.0, 2048.0),
            bounciness: Attr::ranged(0.0, 0.0, 1.0),
            friction_mod: Attr::ranged(1.0, 0.0, 2048.0),
            air_drag_mod: Attr::ranged(1.0, 0.0, 2048.0),
            explosion_kb_resistance: Attr::ranged(0.0, 0.0, 1.0),
            reinforcements: kind.zombie_like().then(|| Attr::ranged(0.0, 0.0, 1.0)),
            extra: Vec::new(),
        };
        let follow = i.follow;
        let mut m = Mob {
            id,
            uuid,
            kind,
            rng,
            pos,
            old: pos,
            vel: [0.0; 3],
            yaw,
            pitch: 0.0,
            head_yaw: yaw,
            pitch_o: 0.0,
            head_yaw_o: yaw,
            body_yaw: yaw,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            fall_distance: 0.0,
            tick_count: 0,
            fire: 0,
            air: 300,
            ticks_frozen: 0,
            water_height: 0.0,
            lava_height: 0.0,
            in_water: false,
            eyes_in_water: false,
            was_eyes_in_water: false,
            support: None,
            piston_deltas: Default::default(),
            move_dist: 0.0,
            next_step: 1.0,
            needs_sync: false,
            removed: false,
            attrs,
            health: i.health as f32,
            hurt_time: 0,
            damage_cooldown: 0,
            last_hurt: 0.0,
            death_time: 0,
            dead: false,
            no_jump_delay: 0,
            jumping: false,
            xxa: 0.0,
            yya: 0.0,
            zza: 0.0,
            speed: 0.0,
            last_hurt_by_mob: None,
            last_hurt_mob: None,
            last_hurt_by_player: 0,
            last_hit: None,
            swing_time: 0,
            use_ticks: -1,
            no_action_time: 0,
            can_break_doors: false,
            ambient_time: 0,
            persistent: false,
            can_pickup: false,
            left_handed: false,
            death_loot: None,
            death_loot_seed: 0,
            no_ai: false,
            invulnerable: false,
            aggressive: false,
            target: None,
            move_ctl: MoveCtl { op: MoveOp::Wait, wanted: pos, speed_mod: 0.0, strafe_fwd: 0.0, strafe_right: 0.0 },
            look: LookCtl::default(),
            body: BodyCtl::default(),
            jump_ctl: false,
            nav: Nav::new(follow),
            goals: Selector::default(),
            targets: Selector::default(),
            sight: Vec::new(),
            equipment: Default::default(),
            drop_chance: [0.085; 8],
            age: 0,
            baby: false,
            in_love: 0,
            love_cause: false,
            swell: 0,
            old_swell: 0,
            swell_dir: -1,
            fuse: 30,
            explosion_radius: 3,
            powered: false,
            ignited: false,
            wool: 0,
            sheared: false,
            eat_anim: 0,
            egg_time: 0,
            chicken_jockey: false,
            carried: None,
            creepy: false,
            stared_at: false,
            target_change_time: 0,
            anger_end: -1,
            anger_target: None,
            tame: false,
            sitting: false,
            cat_sit_pose: false,
            cat_lying: false,
            cat_relax: false,
            owner: None,
            collar: 14,
            variant: 0,
            sound_variant: 0,
            villager: [super::variants::PLAINS_ID, 0, 1],
            climbing: false,
            x: super::variants::Extra::default(),
            cube: super::cubes::CubeState::default(),
            data: Vec::new(),
            track: super::track::Tracker::default(),
            nbt: None,
            custom_name: None,
            surf: super::surface::Surf::default(),
            mount: super::mounts::Mount::default(),
            npc: None,
            golem: super::golems::Golem::default(),
            ill: super::illagers::Ill::default(),
            aqua: super::aquatic::Aqua::default(),
            sea: super::sealife::Aqua::default(),
            bm: super::brainy::Bm::default(),
            wild: super::wild::Wild::default(),
            neth: super::nether::Neth::default(),
            pig: super::piglins::Pig::default(),
            rec: super::recent::Rec::default(),
            end: super::end::EndMob::default(),
            wither: super::wither::Wither::default(),
        };
        if super::recent::handles(kind) {
            super::recent::on_new(&mut m);
        }
        if kind == Kind::Wither {
            super::wither::on_new(&mut m);
        }
        if super::wild::handles(kind) {
            super::wild::on_new(&mut m);
        }
        if super::nether::handles(kind) {
            super::nether::on_new(&mut m);
        }
        if super::piglins::handles(kind) {
            super::piglins::on_new(&mut m);
        }
        if super::end::handles(kind) {
            super::end::on_new(&mut m);
        }
        if super::illagers::handles(kind) {
            super::illagers::on_new(&mut m);
        }
        if super::aquatic::handles(kind) {
            super::aquatic::on_new(&mut m);
        }
        if super::sealife::handles(kind) {
            super::sealife::on_new(&mut m);
        }
        if super::brainy::handles(kind) {
            super::brainy::on_new(&mut m);
        }
        if super::golems::handles(kind) {
            super::golems::on_new(&mut m);
        }
        if kind == Kind::Chicken {
            m.egg_time = m.rng.next_int_bound(6000) + 6000;
            m.surf.flapping = 1.0;
            m.surf.next_flap = 1.0;
        }
        if super::surface::handles(kind) {
            super::surface::on_new(&mut m);
        }
        if super::mounts::handles(kind) {
            super::mounts::on_new(&mut m);
        }
        m
    }

    // ---------- Dimensions ----------

    /// End of anger of a neutral mob: last aggressor, anger target (and its UUID,
    /// species-specific), target and deadline cleared.
    pub fn stop_being_angry(&mut self) {
        self.last_hurt_by_mob = None;
        self.anger_target = None;
        self.golem.angry_at = None;
        self.pig.anger_uuid = None;
        self.surf.angry_at = None;
        self.wild.angry_at = None;
        self.target = None;
        self.anger_end = -1;
    }

    /// Persistent anger against the player `uuid`.
    pub fn angry_at_player(&self, uuid: u128) -> bool {
        self.anger_target == Some(Ent::Player)
            || [self.golem.angry_at, self.pig.anger_uuid, self.surf.angry_at, self.wild.angry_at].contains(&Some(uuid))
    }

    pub fn is_baby(&self) -> bool {
        // F10: the frog is never small (false), even at a negative age.
        if self.kind == Kind::Frog {
            return false;
        }
        if self.kind.ageable() { self.age < 0 } else { self.baby }
    }

    /// Scale of the dimensions (phantom: 1 + 0.15 × size; F09: puffing up, salmon size).
    fn dim_scale(&self) -> f32 {
        if self.kind == Kind::Phantom {
            1.0 + 0.15 * self.x.size as f32
        } else {
            super::sealife::dim_scale(self)
        }
    }

    pub fn width(&self) -> f32 {
        if self.kind.is_cube() {
            return self.cube_dims().0;
        }
        if let Some(d) = super::mounts::pose_dims(self).or_else(|| super::brainy::pose_dims(self)).or_else(|| super::recent::pose_dims(self)) {
            return d.0;
        }
        dims(self.kind, self.is_baby()).0 * self.dim_scale()
    }

    pub fn height(&self) -> f32 {
        if self.kind.is_cube() {
            return self.cube_dims().1;
        }
        if let Some(d) = super::mounts::pose_dims(self).or_else(|| super::brainy::pose_dims(self)).or_else(|| super::recent::pose_dims(self)) {
            return d.1;
        }
        dims(self.kind, self.is_baby()).1 * self.dim_scale()
    }

    pub fn eye_height(&self) -> f64 {
        // F15: shulker eyes stretched by the opening.
        if self.kind == Kind::Shulker {
            return super::end::shulker::eye_height(self);
        }
        if self.kind.is_cube() {
            return f64::from(self.cube_dims().2);
        }
        if let Some(d) = super::mounts::pose_dims(self).or_else(|| super::brainy::pose_dims(self)).or_else(|| super::recent::pose_dims(self)) {
            return f64::from(d.2);
        }
        f64::from(dims(self.kind, self.is_baby()).2 * self.dim_scale())
    }

    pub fn eye_y(&self) -> f64 {
        self.pos[1] + self.eye_height()
    }

    /// Width and height in one pass (same expressions as `width` then `height`).
    pub fn dims_wh(&self) -> (f32, f32) {
        if self.kind.is_cube() {
            let d = self.cube_dims();
            return (d.0, d.1);
        }
        if let Some(d) = super::mounts::pose_dims(self)
            .or_else(|| super::brainy::pose_dims(self))
            .or_else(|| super::recent::pose_dims(self))
        {
            return (d.0, d.1);
        }
        let d = dims(self.kind, self.is_baby());
        let s = self.dim_scale();
        (d.0 * s, d.1 * s)
    }

    pub fn bbox(&self) -> Aabb {
        // F15: shulker box (opening, attachment face).
        if let Some(b) = super::end::bbox(self) {
            return b;
        }
        if dims_once() {
            let (w, h) = self.dims_wh();
            return Self::bbox_at(self.pos, w, h);
        }
        Self::bbox_at(self.pos, self.width(), self.height())
    }

    /// `bbox().intersects(area)`, box computed only if the species' bound allows it.
    #[inline]
    pub fn touches(&self, area: &Aabb) -> bool {
        self.may_touch(area) && self.bbox().intersects(area)
    }

    /// Exact prefilter of the boxes: false only if the mob's box cannot
    /// touch `bb` (static bound of the species, `species::reach`); true otherwise, including for
    /// species with a variable box. `RUSTCRAFT_OPT_PUSHFILTER=0`: always true.
    #[inline]
    pub fn may_touch(&self, bb: &Aabb) -> bool {
        let Some((r, h)) = super::species::reach(self.kind) else { return true };
        if !push_filter() {
            return true;
        }
        #[cfg(test)]
        if super::index::verify() {
            let b = self.bbox();
            let p = self.pos;
            assert!(
                b.min[0] >= p[0] - r
                    && b.max[0] <= p[0] + r
                    && b.min[2] >= p[2] - r
                    && b.max[2] <= p[2] + r
                    && b.min[1] >= p[1]
                    && b.max[1] <= p[1] + h,
                "static bound of {:?} exceeded: {b:?} around {p:?} (r {r}, h {h})",
                self.kind
            );
        }
        let p = self.pos;
        !(p[0] - r >= bb.max[0]
            || p[0] + r <= bb.min[0]
            || p[2] - r >= bb.max[2]
            || p[2] + r <= bb.min[2]
            || p[1] >= bb.max[1]
            || p[1] + h <= bb.min[1])
    }

    pub fn bbox_at(p: [f64; 3], w: f32, h: f32) -> Aabb {
        let hw = f64::from(w) / 2.0;
        Aabb { min: [p[0] - hw, p[1], p[2] - hw], max: [p[0] + hw, p[1] + f64::from(h), p[2] + hw] }
    }

    pub fn block_pos(&self) -> [i32; 3] {
        super::mth::block_of(self.pos)
    }

    pub fn alive(&self) -> bool {
        !self.removed && self.health > 0.0
    }

    /// Invisible (invisibility effect).
    pub fn invisible(&self) -> bool {
        self.x.effects.iter().any(|e| e.id == "minecraft:invisibility")
    }

    /// Love and the player who caused it forgotten.
    pub fn reset_love(&mut self) {
        self.in_love = 0;
        self.love_cause = false;
    }

    pub fn max_health(&self) -> f32 {
        self.attrs.max_health.value() as f32
    }

    pub fn movement_speed(&self) -> f64 {
        self.attrs.speed.value()
    }

    pub fn follow_range(&self) -> f64 {
        self.attrs.follow.value()
    }

    pub fn step_height(&self) -> f64 {
        if self.kind == Kind::SulfurCube && self.has_body_item() {
            return 0.0;
        }
        self.attrs.step.value()
    }

    pub fn on_fire(&self) -> bool {
        self.fire > 0
    }

    pub fn is_in_love(&self) -> bool {
        self.in_love > 0
    }

    // ---------- Metadata ----------

    pub(crate) fn define(&mut self, index: u8, default: Meta) {
        self.data.push(Synced { index, value: default.clone(), default, dirty: false });
    }

    pub fn set(&mut self, index: u8, value: Meta) {
        if let Some(s) = self.data.iter_mut().find(|s| s.index == index)
            && s.value != value
        {
            s.value = value;
            s.dirty = true;
        }
    }

    pub fn get(&self, index: u8) -> Option<&Meta> {
        self.data.iter().find(|s| s.index == index).map(|s| &s.value)
    }

    /// Values defined in the order of the game's classes (indices of `S4-notes`).
    pub fn define_data(&mut self) {
        self.data.clear();
        let none_text = Meta::Text(None);
        self.define(0, Meta::Byte(0));
        self.define(1, Meta::Int(300));
        self.define(2, none_text);
        self.define(3, Meta::Bool(false));
        self.define(4, Meta::Bool(false));
        self.define(5, Meta::Bool(false));
        self.define(6, Meta::Pose(0));
        self.define(7, Meta::Int(0));
        self.define(8, Meta::Byte(0));
        self.define(9, Meta::Float(1.0));
        self.define(10, Meta::Particles(Vec::new()));
        self.define(11, Meta::Bool(false));
        self.define(12, Meta::Int(0));
        self.define(13, Meta::Int(0));
        self.define(14, Meta::BlockPos(None));
        self.define(15, Meta::Byte(0));
        match self.kind {
            k if super::golems::handles(k) => super::golems::define_data(self),
            Kind::Guardian | Kind::ElderGuardian => super::aquatic::define_data(self),
            k if super::surface::handles(k) => super::surface::define_data(self),
            k if super::mounts::handles(k) => super::mounts::define_data(self),
            k if super::illagers::handles(k) => super::illagers::define_data(self),
            k if super::sealife::handles(k) => super::sealife::define_data(self),
            k if super::brainy::handles(k) => super::brainy::define_data(self),
            k if super::wild::handles(k) => super::wild::define_data(self),
            k if super::nether::handles(k) => super::nether::define_data(self),
            k if super::piglins::handles(k) => super::piglins::define_data(self),
            k if super::recent::handles(k) => super::recent::define_data(self),
            k if super::end::handles(k) => super::end::define_data(self),
            Kind::Wither => super::wither::define_data(self),
            Kind::Zombie | Kind::Husk | Kind::ZombieVillager | Kind::Drowned => {
                self.define(16, Meta::Bool(false));
                self.define(17, Meta::Int(0));
                self.define(18, Meta::Bool(false));
                if self.kind == Kind::ZombieVillager {
                    // Profession drawn by the entity's random source at construction (15 professions).
                    let prof = self.rng.next_int_bound(15);
                    if !self.x.vdata_final {
                        self.villager = [super::variants::PLAINS_ID, prof, 1];
                    }
                    self.define(19, Meta::Bool(false));
                    // Initial value = drawn data (the client only receives it if it changes).
                    self.define(20, Meta::Villager([super::variants::PLAINS_ID, prof, 1]));
                    self.define(21, Meta::Bool(false));
                }
            }
            Kind::Skeleton | Kind::Bogged => self.define(16, Meta::Bool(false)),
            Kind::Stray | Kind::Parched | Kind::Giant | Kind::Silverfish | Kind::Endermite => {}
            Kind::Phantom => self.define(16, Meta::Int(0)),
            Kind::Creeper => {
                self.define(16, Meta::Int(-1));
                self.define(17, Meta::Bool(false));
                self.define(18, Meta::Bool(false));
            }
            Kind::Spider | Kind::CaveSpider => self.define(16, Meta::Byte(0)),
            Kind::Enderman => {
                self.define(16, Meta::BlockState(None));
                self.define(17, Meta::Bool(false));
                self.define(18, Meta::Bool(false));
            }
            _ => {
                self.define(16, Meta::Bool(false));
                self.define(17, Meta::Bool(false));
                match self.kind {
                    Kind::Pig => self.define(18, Meta::Int(0)),
                    Kind::Cow | Kind::Chicken => {}
                    Kind::Sheep => self.define(18, Meta::Byte(0)),
                    Kind::Villager => {
                        self.define(18, Meta::Int(0));
                        self.define(19, Meta::Villager([super::variants::PLAINS_ID, 0, 1]));
                        self.define(20, Meta::Bool(false));
                    }
                    Kind::WanderingTrader => self.define(18, Meta::Int(0)),
                    // Declaration order of the game's fields (the index depends on it), full collar.
                    Kind::Wolf => {
                        self.define(18, Meta::Byte(0));
                        self.define(19, Meta::Entity(None));
                        self.define(WOLF_INTERESTED, Meta::Bool(false));
                        self.define(21, Meta::Int(14));
                        self.define(22, Meta::Long(-1));
                    }
                    Kind::Cat => {
                        self.define(18, Meta::Byte(0));
                        self.define(19, Meta::Entity(None));
                        self.define(21, Meta::Bool(false));
                        self.define(22, Meta::Bool(false));
                        self.define(23, Meta::Int(14));
                    }
                    Kind::Slime | Kind::MagmaCube | Kind::SulfurCube => {
                        self.define(18, Meta::Int(1));
                        if self.kind == Kind::SulfurCube {
                            self.define(19, Meta::Bool(false));
                            self.define(20, Meta::Int(-1));
                        }
                    }
                    _ => {}
                }
                // Variant and sound variant: default values set at registry
                // resolution (`variants::variant_init`).
                if let Some([(vi, vs), (si, ss)]) = variant_slots(self.kind) {
                    self.define(vi, Meta::Variant(0, vs));
                    self.define(si, Meta::Variant(0, ss));
                    self.data.sort_by_key(|s| s.index);
                }
            }
        }
        self.data.iter_mut().for_each(|s| s.dirty = false);
    }

    /// Copies the current state into the synced values (marked if changed).
    pub fn sync_data(&mut self) {
        let mut flags = 0i8;
        if self.on_fire() {
            flags |= 1;
        }
        // Effects (S7): invisibility (bit 5) and glowing (bit 6, `Glowing` flag or effect).
        // Cat, ocelot: sprint (bit 3) set by their own AI step.
        if self.surf.sprinting {
            flags |= 8;
        }
        if self.invisible() {
            flags |= 0x20;
        }
        if self.x.glowing || self.x.effects.iter().any(|e| e.id == "minecraft:glowing") {
            flags |= 0x40;
        }
        self.set(0, Meta::Byte(flags));
        // Name always shown, silent, no gravity.
        self.set(3, Meta::Bool(self.x.name_visible));
        self.set(4, Meta::Bool(self.x.silent));
        self.set(5, Meta::Bool(self.surf.no_gravity));
        self.set(1, Meta::Int(self.air));
        self.set(7, Meta::Int(self.ticks_frozen));
        self.set(2, Meta::Text(self.custom_name.clone()));
        // F10: own pose (jump, croak, tongue) set directly, without going back through 0
        // (otherwise the value would be resent every tick).
        let pose = if self.dead {
            7
        } else if let Some(p) = super::end::pose(self) {
            p // F15: dragon struck dead (still flying)
        } else if super::brainy::handles(self.kind) || super::recent::handles(self.kind) {
            self.bm.pose
        } else if self.kind == Kind::Cat && self.surf.crouching {
            5
        } else {
            0
        };
        self.set(6, Meta::Pose(pose));
        self.set(9, Meta::Float(self.health));
        let mut mf = 0i8;
        if self.no_ai {
            mf |= 1;
        }
        if self.left_handed {
            mf |= 2;
        }
        if self.aggressive {
            mf |= 4;
        }
        self.set(15, Meta::Byte(mf));
        match self.kind {
            Kind::Zombie | Kind::Husk | Kind::ZombieVillager | Kind::Drowned => {
                self.set(16, Meta::Bool(self.baby));
                self.set(18, Meta::Bool(self.x.conv_time >= 0));
                if self.kind == Kind::ZombieVillager {
                    self.set(19, Meta::Bool(self.x.cure_time >= 0));
                    self.set(20, Meta::Villager(self.villager));
                    self.set(21, Meta::Bool(self.x.vdata_final));
                }
            }
            Kind::Skeleton => self.set(16, Meta::Bool(self.x.conv_time >= 0)),
            Kind::Bogged => self.set(16, Meta::Bool(self.sheared)),
            Kind::Phantom => self.set(16, Meta::Int(self.x.size)),
            Kind::Stray | Kind::Parched | Kind::Giant | Kind::Silverfish | Kind::Endermite => {}
            Kind::Creeper => {
                self.set(16, Meta::Int(self.swell_dir));
                self.set(17, Meta::Bool(self.powered));
                self.set(18, Meta::Bool(self.ignited));
            }
            Kind::Spider | Kind::CaveSpider => self.set(16, Meta::Byte(i8::from(self.climbing))),
            Kind::Enderman => {
                self.set(16, Meta::BlockState(self.carried.map(|s| s as i32)));
                self.set(17, Meta::Bool(self.creepy));
                self.set(18, Meta::Bool(self.stared_at));
            }
            k if super::golems::handles(k) => super::golems::sync_data(self),
            k if super::surface::handles(k) => super::surface::sync_data(self),
            k if super::mounts::handles(k) => super::mounts::sync_data(self),
            k if super::illagers::handles(k) => super::illagers::sync_data(self),
            k if super::sealife::handles(k) => super::sealife::sync_data(self),
            k if super::brainy::handles(k) => super::brainy::sync_data(self),
            k if super::wild::handles(k) => super::wild::sync_data(self),
            k if super::nether::handles(k) => super::nether::sync_data(self),
            k if super::piglins::handles(k) => super::piglins::sync_data(self),
            k if super::recent::handles(k) => super::recent::sync_data(self),
            k if super::end::handles(k) => super::end::sync_data(self),
            Kind::Wither => super::wither::sync_data(self),
            _ => {
                self.set(16, Meta::Bool(self.age < 0));
                self.set(17, Meta::Bool(self.mount.age_locked));
                match self.kind {
                    Kind::Sheep => self.set(18, Meta::Byte((self.wool & 15) as i8 | if self.sheared { 16 } else { 0 })),
                    Kind::Villager => {
                        self.set(19, Meta::Villager(self.villager));
                        self.set(20, Meta::Bool(self.x.vdata_final));
                    }
                    Kind::Wolf | Kind::Cat => {
                        let pose = self.sitting || self.cat_sit_pose;
                        let f = i8::from(pose) | if self.tame { 4 } else { 0 };
                        self.set(18, Meta::Byte(f));
                        self.set(19, Meta::Entity(self.owner));
                        if self.kind == Kind::Cat {
                            self.set(21, Meta::Bool(self.cat_lying));
                            self.set(22, Meta::Bool(self.cat_relax));
                            self.set(23, Meta::Int(self.collar));
                        } else {
                            self.set(21, Meta::Int(self.collar));
                            self.set(22, Meta::Long(self.anger_end));
                        }
                    }
                    Kind::Slime | Kind::MagmaCube | Kind::SulfurCube => self.cube_sync_data(),
                    _ => {}
                }
                if let Some([(vi, vs), (si, ss)]) = variant_slots(self.kind) {
                    self.set(vi, Meta::Variant(self.variant, vs));
                    self.set(si, Meta::Variant(self.sound_variant, ss));
                }
            }
        }
        super::villager::sync_data(self); // F05: traders
        super::aquatic::sync_data(self); // F08: swimming, trident, beam
    }

    /// Non-default values (pairing).
    pub fn non_default(&self) -> Vec<(u8, Meta)> {
        self.data.iter().filter(|s| s.value != s.default).map(|s| (s.index, s.value.clone())).collect()
    }

    /// Values changed since the last send.
    pub fn take_dirty(&mut self) -> Vec<(u8, Meta)> {
        let mut out = Vec::new();
        for s in self.data.iter_mut().filter(|s| s.dirty) {
            s.dirty = false;
            out.push((s.index, s.value.clone()));
        }
        out
    }

    // ---------- NBT ----------

    /// Fields read from a world entity (`entities/`).
    pub fn load(&mut self, e: &Nbt) {
        if let Some(p) = nbtio::f64s::<3>(e.get("Pos")) {
            self.pos = p;
            self.old = p;
        }
        if let Some(v) = nbtio::motion(e.get("Motion")) {
            self.vel = v;
        }
        if let Some([y, p]) = nbtio::f32s::<2>(e.get("Rotation")) {
            self.yaw = y;
            self.pitch = p;
            self.head_yaw = y;
            self.body_yaw = y;
        }
        let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
        self.on_ground = flag("OnGround");
        self.fall_distance = e.get("fall_distance").and_then(f64_of).unwrap_or(0.0);
        self.fire = int(e, "Fire").map_or(0, |v| v as i32);
        self.air = int(e, "Air").map_or(300, |v| v as i32);
        self.ticks_frozen = int(e, "TicksFrozen").unwrap_or(0) as i32;
        if let Some(h) = nbtio::float(e, "Health") {
            self.health = h;
        }
        self.hurt_time = int(e, "HurtTime").unwrap_or(0) as i32;
        self.death_time = int(e, "DeathTime").unwrap_or(0) as i32;
        self.persistent = flag("PersistenceRequired");
        self.can_pickup = flag("CanPickUpLoot");
        self.left_handed = flag("LeftHanded");
        self.death_loot = e.get("DeathLootTable").and_then(Nbt::as_str).map(crate::commands::args::id);
        self.death_loot_seed = e.get("DeathLootTableSeed").and_then(Nbt::as_i64).unwrap_or(0);
        self.no_ai = flag("NoAI");
        self.invulnerable = flag("Invulnerable");
        self.custom_name = e.get("CustomName").cloned();
        if let Some(u) = nbtio::uuid(e.get("UUID")) {
            self.uuid = u;
        }
        self.age = int(e, "Age").unwrap_or(0) as i32;
        self.mount.age_locked = flag("AgeLocked");
        self.in_love = int(e, "InLove").unwrap_or(0) as i32;
        self.baby = flag("IsBaby");
        if let Some(Nbt::Compound(eq)) = e.get("equipment") {
            for (k, v) in eq {
                if let Some(i) = slot_index(k) {
                    self.equipment[i] = Stack::from_nbt(v);
                }
            }
        }
        self.drop_chance = [0.085; 8];
        if let Some(Nbt::Compound(d)) = e.get("drop_chances") {
            for (k, v) in d {
                if let (Some(i), Some(c)) = (slot_index(k), f64_of(v)) {
                    self.drop_chance[i] = c as f32;
                }
            }
        }
        // Cubes: the size sets the attributes before they are reread (game order).
        if self.kind.is_cube() {
            self.cube_load_size(e);
        }
        // Registered attributes: permanent bases and modifiers (spawn bonus of the
        // zombies, `/attribute`…), unmodeled attributes kept as they are.
        if let Some(Nbt::List(list)) = e.get("attributes") {
            self.attrs.load(list);
        }
        match self.kind {
            Kind::Creeper => {
                self.fuse = int(e, "Fuse").map_or(30, |v| v as i32);
                self.explosion_radius = int(e, "ExplosionRadius").map_or(3, |v| v as i32);
                self.powered = flag("powered");
                self.ignited = flag("ignited");
            }
            Kind::Sheep => {
                self.wool = int(e, "Color").unwrap_or(0) as u8;
                self.sheared = flag("Sheared");
            }
            Kind::Bogged => self.sheared = flag("sheared"),
            Kind::Chicken => {
                self.chicken_jockey = flag("IsChickenJockey");
                if let Some(t) = int(e, "EggLayTime") {
                    self.egg_time = t as i32;
                }
            }
            Kind::Wolf | Kind::Cat => {
                self.sitting = flag("Sitting");
                self.owner = nbtio::uuid(e.get("Owner"));
                self.tame = self.owner.is_some();
                // Read without an owner: back to the wild state with its effects; the wolf
                // takes back 8 base max HP whatever the data (game 26.3 default,
                // reproduced), health follows at the attribute refresh.
                if self.kind == Kind::Wolf && !self.tame {
                    self.attrs.max_health.base = 8.0;
                    self.health = self.health.min(self.max_health());
                }
                self.collar = int(e, "CollarColor").map_or(14, |v| v as i32);
                if let Some(t) = e.get("anger_end_time").and_then(f64_of) {
                    self.anger_end = t as i64;
                }
            }
            k if super::golems::handles(k) => super::golems::load(self, e),
            k if super::aquatic::handles(k) => super::aquatic::load(self, e),
            k if super::surface::handles(k) => super::surface::load(self, e),
            k if super::mounts::handles(k) => super::mounts::load(self, e),
            k if super::illagers::handles(k) => super::illagers::load(self, e),
            k if super::sealife::handles(k) => super::sealife::load(self, e),
            k if super::brainy::handles(k) => super::brainy::load(self, e),
            k if super::wild::handles(k) => super::wild::load(self, e),
            k if super::nether::handles(k) => super::nether::load(self, e),
            k if super::piglins::handles(k) => super::piglins::load(self, e),
            k if super::recent::handles(k) => super::recent::load(self, e),
            k if super::end::handles(k) => super::end::load(self, e),
            Kind::Wither => super::wither::load(self, e),
            Kind::Slime | Kind::MagmaCube | Kind::SulfurCube => self.cube_load(e),
            _ => {}
        }
        super::variants::load_extra(self, e);
        super::villager::load(self, e);
        self.nbt = Some(e.clone());
    }

    /// Compound to write: the compound read, managed fields replaced.
    pub fn save(&self) -> Nbt {
        let mut e = self.nbt.clone().unwrap_or(Nbt::Compound(Vec::new()));
        let dbl = |v: [f64; 3]| Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect());
        let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
        set(&mut e, "id", Nbt::String(self.kind.info().name.into()));
        set(&mut e, "Pos", dbl(self.pos));
        set(&mut e, "Motion", dbl(self.vel));
        set(&mut e, "Rotation", Nbt::List(vec![Nbt::Float(self.yaw), Nbt::Float(self.pitch)]));
        set(&mut e, "OnGround", Nbt::Byte(i8::from(self.on_ground)));
        set(&mut e, "fall_distance", Nbt::Double(self.fall_distance));
        set(&mut e, "Fire", Nbt::Short(self.fire as i16));
        set(&mut e, "Air", Nbt::Short(self.air as i16));
        if self.ticks_frozen > 0 {
            set(&mut e, "TicksFrozen", Nbt::Int(self.ticks_frozen));
        } else {
            nbtio::remove(&mut e, "TicksFrozen");
        }
        set(&mut e, "UUID", nbtio::uuid_nbt(self.uuid));
        set(&mut e, "Health", Nbt::Float(self.health));
        set(&mut e, "HurtTime", Nbt::Short(self.hurt_time as i16));
        set(&mut e, "DeathTime", Nbt::Short(self.death_time as i16));
        set(&mut e, "HurtByTimestamp", Nbt::Int(self.last_hurt_by_mob.map_or(0, |h| h.1)));
        set(&mut e, "PersistenceRequired", Nbt::Byte(i8::from(self.persistent)));
        set(&mut e, "CanPickUpLoot", Nbt::Byte(i8::from(self.can_pickup)));
        set(&mut e, "LeftHanded", Nbt::Byte(i8::from(self.left_handed)));
        match &self.death_loot {
            Some(t) => set(&mut e, "DeathLootTable", Nbt::String(t.clone())),
            None => crate::nbtio::remove(&mut e, "DeathLootTable"),
        }
        if self.death_loot_seed != 0 {
            set(&mut e, "DeathLootTableSeed", Nbt::Long(self.death_loot_seed));
        } else {
            crate::nbtio::remove(&mut e, "DeathLootTableSeed");
        }
        if self.no_ai {
            set(&mut e, "NoAI", Nbt::Byte(1));
        }
        set(&mut e, "Invulnerable", Nbt::Byte(i8::from(self.invulnerable)));
        if let Some(n) = &self.custom_name {
            set(&mut e, "CustomName", n.clone());
        }
        let eq: Vec<(String, Nbt)> = self
            .equipment
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((SLOT_NAMES[i].to_owned(), s.as_ref()?.to_nbt(None))))
            .collect();
        if eq.is_empty() {
            nbtio::remove(&mut e, "equipment");
        } else {
            set(&mut e, "equipment", Nbt::Compound(eq));
        }
        // Non-default drop chances (drowned's shell, saddle, armor placed…).
        let chances: Vec<(String, Nbt)> = self
            .drop_chance
            .iter()
            .enumerate()
            .filter(|(_, c)| **c != 0.085)
            .map(|(i, c)| (SLOT_NAMES[i].to_owned(), Nbt::Float(*c)))
            .collect();
        if chances.is_empty() {
            nbtio::remove(&mut e, "drop_chances");
        } else {
            set(&mut e, "drop_chances", Nbt::Compound(chances));
        }
        set(&mut e, "attributes", Nbt::List(self.attrs.save()));
        if self.kind.ageable() {
            set(&mut e, "Age", Nbt::Int(self.age));
            set(&mut e, "ForcedAge", Nbt::Int(self.x.forced_age));
            set(&mut e, "AgeLocked", Nbt::Byte(i8::from(self.mount.age_locked)));
            set(&mut e, "InLove", Nbt::Int(self.in_love));
        }
        match self.kind {
            Kind::Zombie | Kind::Husk | Kind::ZombieVillager | Kind::Drowned => {
                set(&mut e, "IsBaby", Nbt::Byte(i8::from(self.baby)))
            }
            Kind::Creeper => {
                set(&mut e, "Fuse", Nbt::Short(self.fuse as i16));
                set(&mut e, "ExplosionRadius", Nbt::Byte(self.explosion_radius as i8));
                set(&mut e, "powered", Nbt::Byte(i8::from(self.powered)));
                set(&mut e, "ignited", Nbt::Byte(i8::from(self.ignited)));
            }
            Kind::Sheep => {
                set(&mut e, "Color", Nbt::Byte(self.wool as i8));
                set(&mut e, "Sheared", Nbt::Byte(i8::from(self.sheared)));
            }
            Kind::Chicken => {
                set(&mut e, "IsChickenJockey", Nbt::Byte(i8::from(self.chicken_jockey)));
                set(&mut e, "EggLayTime", Nbt::Int(self.egg_time));
            }
            Kind::Bogged => set(&mut e, "sheared", Nbt::Byte(i8::from(self.sheared))),
            Kind::Wolf | Kind::Cat => {
                set(&mut e, "Sitting", Nbt::Byte(i8::from(self.sitting)));
                set(&mut e, "CollarColor", Nbt::Byte(self.collar as i8));
                if let Some(o) = self.owner {
                    set(&mut e, "Owner", nbtio::uuid_nbt(o));
                }
                if self.kind == Kind::Wolf && self.anger_end >= 0 {
                    set(&mut e, "anger_end_time", Nbt::Long(self.anger_end));
                }
            }
            k if super::golems::handles(k) => super::golems::save(self, &mut e),
            k if super::aquatic::handles(k) => super::aquatic::save(self, &mut e),
            k if super::surface::handles(k) => super::surface::save(self, &mut e),
            k if super::mounts::handles(k) => super::mounts::save(self, &mut e),
            k if super::illagers::handles(k) => super::illagers::save(self, &mut e),
            k if super::sealife::handles(k) => super::sealife::save(self, &mut e),
            k if super::brainy::handles(k) => super::brainy::save(self, &mut e),
            k if super::wild::handles(k) => super::wild::save(self, &mut e),
            k if super::nether::handles(k) => super::nether::save(self, &mut e),
            k if super::piglins::handles(k) => super::piglins::save(self, &mut e),
            k if super::recent::handles(k) => super::recent::save(self, &mut e),
            k if super::end::handles(k) => super::end::save(self, &mut e),
            Kind::Wither => super::wither::save(self, &mut e),
            Kind::Slime | Kind::MagmaCube | Kind::SulfurCube => self.cube_save(&mut e),
            _ => {}
        }
        super::variants::save_extra(self, &mut e);
        super::villager::save(self, &mut e);
        e
    }
}

/// Modeled attributes that the game syncs with the client.
pub(crate) const SYNCABLE_MODELED: [&str; 12] = [
    "minecraft:max_health",
    "minecraft:movement_speed",
    "minecraft:armor",
    "minecraft:step_height",
    "minecraft:gravity",
    "minecraft:jump_strength",
    "minecraft:safe_fall_distance",
    "minecraft:fall_damage_multiplier",
    "minecraft:bounciness",
    "minecraft:friction_modifier",
    "minecraft:air_drag_modifier",
    "minecraft:explosion_knockback_resistance",
];

/// Synced attributes that the simulation keeps as they are (`extra` list).
pub(crate) const SYNCABLE_EXTRA: [&str; 20] = [
    "minecraft:armor_toughness",
    "minecraft:attack_speed",
    "minecraft:below_name_distance",
    "minecraft:block_break_speed",
    "minecraft:block_interaction_range",
    "minecraft:burning_time",
    "minecraft:camera_distance",
    "minecraft:entity_interaction_range",
    "minecraft:flying_speed",
    "minecraft:luck",
    "minecraft:max_absorption",
    "minecraft:mining_efficiency",
    "minecraft:movement_efficiency",
    "minecraft:name_tag_distance",
    "minecraft:oxygen_bonus",
    "minecraft:scale",
    "minecraft:sneaking_speed",
    "minecraft:submerged_mining_speed",
    "minecraft:sweeping_damage_ratio",
    "minecraft:water_movement_efficiency",
];

/// Fingerprints of the syncable attributes, one per attribute of `SYNCABLE_MODELED` (same order)
/// then one for all the kept attributes: a change of base or modifiers alters
/// that of the attribute (negligible collision), without allocation.
pub(crate) type AttrPrints = [u64; SYNCABLE_MODELED.len() + 1];

pub(crate) fn syncable_fingerprint(a: &Attrs) -> AttrPrints {
    let mut out = [0u64; SYNCABLE_MODELED.len() + 1];
    let mix = |h: &mut u64, v: u64| {
        *h ^= v;
        *h = h.wrapping_mul(0x0100_0000_01b3).rotate_left(29);
    };
    // Direct fields (same list as `SYNCABLE_MODELED`): no search by name on every tick.
    let fields = [
        &a.max_health,
        &a.speed,
        &a.armor,
        &a.step,
        &a.gravity,
        &a.jump,
        &a.safe_fall,
        &a.fall_mult,
        &a.bounciness,
        &a.friction_mod,
        &a.air_drag_mod,
        &a.explosion_kb_resistance,
    ];
    for (attr, h) in fields.into_iter().zip(&mut out) {
        *h = 0xcbf2_9ce4_8422_2325;
        mix(h, attr.base.to_bits());
        for (id, amount, op) in &attr.mods {
            mix(h, id.as_ptr() as u64 ^ (id.len() as u64) << 48);
            mix(h, amount.to_bits() ^ *op as u64);
        }
        mix(h, attr.mods.len() as u64);
    }
    // Kept attributes: rare, changed only by command or reread.
    let h = &mut out[SYNCABLE_MODELED.len()];
    *h = 0xcbf2_9ce4_8422_2325;
    for e in &a.extra {
        mix(h, e.get("base").and_then(f64_of).map_or(0, f64::to_bits));
        for md in nbtio::list(e.get("modifiers")) {
            mix(h, md.get("amount").and_then(f64_of).map_or(0, f64::to_bits));
        }
        mix(h, e.get("id").and_then(Nbt::as_str).map_or(0, |s| s.len() as u64));
    }
    mix(h, a.extra.len() as u64);
    out
}

/// Wolf: "interested" (head tilted in front of food).
pub(crate) const WOLF_INTERESTED: u8 = 20;

/// Index and serializers (variant, sound variant) of the species whose variants are
/// registry entries.
pub(crate) fn variant_slots(kind: Kind) -> Option<[(u8, u8); 2]> {
    Some(match kind {
        Kind::Pig => [(19, 28), (20, 29)],
        Kind::Cow => [(18, 23), (19, 24)],
        Kind::Chicken => [(18, 30), (19, 31)],
        Kind::Wolf => [(23, 25), (24, 26)],
        Kind::Cat => [(20, 21), (24, 22)],
        _ => return None,
    })
}

/// Equipment slot names (network order: main hand, offhand, feet, legs, chest,
/// head, body, saddle).
pub const SLOT_NAMES: [&str; 8] = ["mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle"];

pub fn slot_index(name: &str) -> Option<usize> {
    SLOT_NAMES.iter().position(|s| *s == name)
}

/// Any NBT number as `f64`.
pub fn f64_of(v: &Nbt) -> Option<f64> {
    match v {
        Nbt::Double(d) => Some(*d),
        Nbt::Float(f) => Some(f64::from(*f)),
        other => other.as_i64().map(|i| i as f64),
    }
}
