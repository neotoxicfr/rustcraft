//! Illagers, patrols and raids (family F07, `server-mobs.md` § 4.F07): pillager,
//! vindicator, evoker, vex, illusioner, ravager, witch. Info sheets, own state
//! (patrol, raid, spells, crossbow, potions), metadata, NBT, goals and hooks called by
//! the common engine (`mobs/`). The goals are in `goals.rs`, the abilities in `spells.rs`
//! (evoker, illusioner, fangs), `vex.rs`, `ravager.rs`, `witch.rs` (and thrown potions),
//! `crossbow.rs` (bolts), `patrol.rs` (patrol spawner).

pub(crate) mod crossbow;
pub(crate) mod goals;
pub(crate) mod patrol;
pub(crate) mod raids;
pub(crate) mod ravager;
pub(crate) mod spells;
pub(crate) mod vex;
pub(crate) mod witch;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::goals::{Goal, TBase, Who};
use super::mob::{Ent, Mob};
use super::species::{Category, Info, Kind, info as base_info};
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

pub use goals::IGoal;

// ---------- Info sheets ----------

/// The game's monster: 5 experience points, tracking 8 chunks.
const fn monster(name: &'static str, width: f32, height: f32, eye: f32) -> Info {
    let mut i = base_info(name, Category::Monster, width, height, eye);
    i.xp = 5;
    i.tracking = 8;
    i.not_in_peaceful = true;
    i.hurt = "minecraft:entity.hostile.hurt";
    i.death = "minecraft:entity.hostile.death";
    i
}

/// Illager: 0.6 × 1.95, eyes at 0.85 of the height.
const fn illager(name: &'static str, sounds: [&'static str; 3]) -> Info {
    let mut i = monster(name, 0.6, 1.95, 1.95 * 0.85);
    i.ambient = Some(sounds[0]);
    i.hurt = sounds[1];
    i.death = sounds[2];
    i
}

static PILLAGER: Info = {
    let mut i = illager(
        "minecraft:pillager",
        ["minecraft:entity.pillager.ambient", "minecraft:entity.pillager.hurt", "minecraft:entity.pillager.death"],
    );
    i.health = 24.0;
    i.speed = 0.35f32 as f64;
    i.attack = 5.0;
    i.follow = 32.0;
    i
};

static VINDICATOR: Info = {
    let mut i = illager(
        "minecraft:vindicator",
        [
            "minecraft:entity.vindicator.ambient",
            "minecraft:entity.vindicator.hurt",
            "minecraft:entity.vindicator.death",
        ],
    );
    i.health = 24.0;
    i.speed = 0.35f32 as f64;
    i.attack = 5.0;
    i.follow = 12.0;
    i
};

static EVOKER: Info = {
    let mut i = illager(
        "minecraft:evoker",
        ["minecraft:entity.evoker.ambient", "minecraft:entity.evoker.hurt", "minecraft:entity.evoker.death"],
    );
    i.health = 24.0;
    i.speed = 0.5;
    i.follow = 12.0;
    i.xp = 10;
    i
};

static ILLUSIONER: Info = {
    let mut i = illager(
        "minecraft:illusioner",
        [
            "minecraft:entity.illusioner.ambient",
            "minecraft:entity.illusioner.hurt",
            "minecraft:entity.illusioner.death",
        ],
    );
    i.health = 32.0;
    i.speed = 0.5;
    i.follow = 18.0;
    i
};

static VEX: Info = {
    let mut i = monster("minecraft:vex", 0.4, 0.8, 0.518_75);
    i.health = 14.0;
    i.attack = 4.0;
    i.xp = 3;
    i.ambient = Some("minecraft:entity.vex.ambient");
    i.hurt = "minecraft:entity.vex.hurt";
    i.death = "minecraft:entity.vex.death";
    i.fire_immune = true;
    i
};

static RAVAGER: Info = {
    let mut i = monster("minecraft:ravager", 1.95, 2.2, 2.2 * 0.85);
    i.health = 100.0;
    i.speed = 0.3;
    i.attack = 12.0;
    i.follow = 32.0;
    i.step = 1.0;
    i.xp = 20;
    i.tracking = 10;
    i.ambient = Some("minecraft:entity.ravager.ambient");
    i.hurt = "minecraft:entity.ravager.hurt";
    i.death = "minecraft:entity.ravager.death";
    i.step_sound = Some("minecraft:entity.ravager.step");
    i
};

static WITCH: Info = {
    let mut i = monster("minecraft:witch", 0.6, 1.95, 1.62);
    i.health = 26.0;
    i.speed = 0.25;
    i.ambient = Some("minecraft:entity.witch.ambient");
    i.hurt = "minecraft:entity.witch.hurt";
    i.death = "minecraft:entity.witch.death";
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Pillager => &PILLAGER,
        Kind::Vindicator => &VINDICATOR,
        Kind::Evoker => &EVOKER,
        Kind::Vex => &VEX,
        Kind::Illusioner => &ILLUSIONER,
        Kind::Ravager => &RAVAGER,
        _ => &WITCH,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(
        k,
        Kind::Pillager | Kind::Vindicator | Kind::Evoker | Kind::Vex | Kind::Illusioner | Kind::Ravager | Kind::Witch
    )
}

/// Tag `#minecraft:illager` (and `#illager_friends`, which contains only it).
pub fn is_illager(k: Kind) -> bool {
    matches!(k, Kind::Pillager | Kind::Vindicator | Kind::Evoker | Kind::Illusioner)
}

/// "Raid pillager" class (tag `#minecraft:raiders`).
pub fn is_raider(k: Kind) -> bool {
    is_illager(k) || k == Kind::Ravager || k == Kind::Witch
}

/// Spell caster.
pub fn is_spellcaster(k: Kind) -> bool {
    matches!(k, Kind::Evoker | Kind::Illusioner)
}

// ---------- Own state ----------

/// Spell in progress (the game's network ids).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Spell {
    #[default]
    None = 0,
    SummonVex = 1,
    Fangs = 2,
    Wololo = 3,
    Disappear = 4,
    Blindness = 5,
}

/// State of the family (`ill` field of the mob).
#[derive(Debug, Clone, Default)]
pub struct Ill {
    /// Patrol: distant target, leader, patrolling.
    pub patrol_target: Option<[i32; 3]>,
    pub patrol_leader: bool,
    pub patrolling: bool,
    /// Raid: wave, can join, current raid, celebration, ticks outside the raid.
    pub wave: i32,
    pub can_join_raid: bool,
    pub raid: Option<i32>,
    pub celebrating: bool,
    pub ticks_outside_raid: i32,
    /// Pillager: crossbow loading, 5-slot inventory (white banners in a raid).
    pub charging: bool,
    pub inventory: Vec<Stack>,
    /// Spell casters: remaining casting ticks, displayed spell, targeted sheep.
    pub spell_ticks: i32,
    pub spell: Spell,
    pub wololo: Option<i32>,
    /// Vindicator named "Johnny".
    pub johnny: bool,
    /// Vex: summoner (recorded UUID and resolved id), linked origin, limited life,
    /// charge (flag 1 of index 16).
    pub owner: Option<u128>,
    pub owner_id: Option<i32>,
    pub bound: Option<[i32; 3]>,
    pub life: Option<i32>,
    pub vex_charging: bool,
    /// Ravager: hit (10), stunned (40), roar (20).
    pub attack_tick: i32,
    pub stun_tick: i32,
    pub roar_tick: i32,
    /// Witch: potion drunk (ticks left), drinking, ally healing delay,
    /// player targeting allowed.
    pub using_time: i32,
    pub drinking: bool,
    pub heal_cooldown: i32,
    pub attack_players: bool,
}

/// World state of the family (`ill` field of `Mobs`).
#[derive(Debug, Clone, Default)]
pub struct IllWorld {
    /// Patrol spawner: ticks before the next attempt.
    pub patrol_next: i32,
    /// Evoker fangs (thrown potions are common projectiles).
    pub fangs: Vec<spells::Fangs>,
    pub raids: raids::Raids,
    /// Crossbow loading sounds already played (fields of the game's single item, shared
    /// by all crossbows).
    pub crossbow_start_played: bool,
    pub crossbow_mid_played: bool,
}

/// Construction.
pub fn on_new(m: &mut Mob) {
    match m.kind {
        Kind::Ravager => {
            m.attrs.knockback_resistance.base = 0.75;
            m.attrs.attack_knockback.base = 1.5;
        }
        Kind::Vex => m.attrs.follow.base = 16.0,
        _ => {}
    }
}

pub fn define_data(m: &mut Mob) {
    match m.kind {
        Kind::Vex => m.define(16, Meta::Byte(0)),
        _ => {
            // "Celebrating" data of raid pillagers.
            m.define(16, Meta::Bool(false));
            match m.kind {
                Kind::Pillager | Kind::Witch => m.define(17, Meta::Bool(false)),
                Kind::Evoker | Kind::Illusioner => m.define(17, Meta::Byte(0)),
                _ => {}
            }
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    // Item in use (crossbow drawn, potion drunk): flag 1 of index 8, right hand.
    // The witch never "uses" an item on the living side: her drink goes through index 17.
    let using = m.kind != Kind::Witch && m.use_ticks >= 0;
    m.set(8, Meta::Byte(i8::from(using)));
    // Vex: no gravity from its first tick.
    if m.kind == Kind::Vex {
        if m.tick_count > 0 {
            m.set(5, Meta::Bool(true));
        }
        m.set(16, Meta::Byte(i8::from(m.ill.vex_charging)));
        return;
    }
    m.set(16, Meta::Bool(m.ill.celebrating));
    match m.kind {
        Kind::Pillager => m.set(17, Meta::Bool(m.ill.charging)),
        Kind::Witch => m.set(17, Meta::Bool(m.ill.drinking)),
        Kind::Evoker | Kind::Illusioner => m.set(17, Meta::Byte(m.ill.spell as i8)),
        _ => {}
    }
}

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str, d: bool| int(e, k).map_or(d, |v| v != 0);
    if m.kind != Kind::Vex {
        // The "patrol monster" layer.
        m.ill.patrol_target = block_pos(e.get("patrol_target"));
        m.ill.patrol_leader = flag("PatrolLeader", false);
        m.ill.patrolling = flag("Patrolling", false);
        // Raid pillager (the raid itself is attached by `raids.rs`).
        m.ill.wave = int(e, "Wave").unwrap_or(0) as i32;
        m.ill.can_join_raid = flag("CanJoinRaid", false);
        m.ill.raid = int(e, "RaidId").map(|v| v as i32);
    }
    match m.kind {
        Kind::Pillager => {
            m.ill.inventory = nbtio::list(e.get("Inventory")).iter().filter_map(Stack::from_nbt).collect();
            m.can_pickup = true;
        }
        Kind::Vindicator => {
            m.ill.johnny = flag("Johnny", false);
            // The custom name choice (read back with the entity): a "Johnny" name makes it Johnny forever.
            if m.custom_name.as_ref().map(plain_name).as_deref() == Some("Johnny") {
                m.ill.johnny = true;
            }
        }
        Kind::Evoker | Kind::Illusioner => m.ill.spell_ticks = int(e, "SpellTicks").unwrap_or(0) as i32,
        Kind::Vex => {
            m.ill.bound = block_pos(e.get("bound_pos"));
            m.ill.life = int(e, "life_ticks").map(|v| v as i32);
            m.ill.owner = nbtio::uuid(e.get("owner"));
        }
        Kind::Ravager => {
            m.ill.attack_tick = int(e, "AttackTick").unwrap_or(0) as i32;
            m.ill.stun_tick = int(e, "StunTick").unwrap_or(0) as i32;
            m.ill.roar_tick = int(e, "RoarTick").unwrap_or(0) as i32;
        }
        _ => {}
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    if m.kind != Kind::Vex {
        match m.ill.patrol_target {
            Some(p) => set(e, "patrol_target", Nbt::IntArray(p.to_vec())),
            None => nbtio::remove(e, "patrol_target"),
        }
        set(e, "PatrolLeader", Nbt::Byte(i8::from(m.ill.patrol_leader)));
        set(e, "Patrolling", Nbt::Byte(i8::from(m.ill.patrolling)));
        set(e, "Wave", Nbt::Int(m.ill.wave));
        set(e, "CanJoinRaid", Nbt::Byte(i8::from(m.ill.can_join_raid)));
        match m.ill.raid {
            Some(id) => set(e, "RaidId", Nbt::Int(id)),
            None => nbtio::remove(e, "RaidId"),
        }
    }
    match m.kind {
        Kind::Pillager => {
            let items: Vec<Nbt> = m.ill.inventory.iter().map(|s| s.to_nbt(None)).collect();
            if items.is_empty() {
                nbtio::remove(e, "Inventory");
            } else {
                set(e, "Inventory", Nbt::List(items));
            }
        }
        Kind::Vindicator => {
            if m.ill.johnny {
                set(e, "Johnny", Nbt::Byte(1));
            }
        }
        Kind::Evoker | Kind::Illusioner => set(e, "SpellTicks", Nbt::Int(m.ill.spell_ticks)),
        Kind::Vex => {
            match m.ill.bound {
                Some(p) => set(e, "bound_pos", Nbt::IntArray(p.to_vec())),
                None => nbtio::remove(e, "bound_pos"),
            }
            match m.ill.life {
                Some(t) => set(e, "life_ticks", Nbt::Int(t)),
                None => nbtio::remove(e, "life_ticks"),
            }
            match m.ill.owner {
                Some(u) => set(e, "owner", nbtio::uuid_nbt(u)),
                None => nbtio::remove(e, "owner"),
            }
        }
        Kind::Ravager => {
            set(e, "AttackTick", Nbt::Int(m.ill.attack_tick));
            set(e, "StunTick", Nbt::Int(m.ill.stun_tick));
            set(e, "RoarTick", Nbt::Int(m.ill.roar_tick));
        }
        _ => {}
    }
}

/// Recorded block position (array of 3 integers).
fn block_pos(v: Option<&Nbt>) -> Option<[i32; 3]> {
    match v? {
        Nbt::IntArray(a) if a.len() == 3 => Some([a[0], a[1], a[2]]),
        _ => None,
    }
}

// ---------- Captain banner ----------

/// Ominous banner: white banner, 8 patterns, name, rarity.
pub fn ominous_banner() -> Stack {
    let pat = |p: &str, c: &str| {
        Nbt::Compound(vec![("pattern".into(), Nbt::String(p.into())), ("color".into(), Nbt::String(c.into()))])
    };
    let patterns = Nbt::List(vec![
        pat("minecraft:rhombus", "cyan"),
        pat("minecraft:stripe_bottom", "light_gray"),
        pat("minecraft:stripe_center", "gray"),
        pat("minecraft:border", "light_gray"),
        pat("minecraft:stripe_middle", "black"),
        pat("minecraft:half_horizontal", "light_gray"),
        pat("minecraft:circle", "light_gray"),
        pat("minecraft:border", "black"),
    ]);
    let comps = Nbt::Compound(vec![
        ("minecraft:banner_patterns".into(), patterns),
        (
            "minecraft:tooltip_display".into(),
            Nbt::Compound(vec![(
                "hidden_components".into(),
                Nbt::List(vec![Nbt::String("minecraft:banner_patterns".into())]),
            )]),
        ),
        (
            "minecraft:item_name".into(),
            Nbt::Compound(vec![("translate".into(), Nbt::String("block.minecraft.ominous_banner".into()))]),
        ),
        ("minecraft:rarity".into(), Nbt::String("uncommon".into())),
    ]);
    Stack { id: "minecraft:white_banner".into(), count: 1, components: Some(comps) }
}

/// The comparison of two item stacks with the ominous banner (same item, same components, same count).
pub fn is_ominous_banner(s: &Stack) -> bool {
    let b = ominous_banner();
    s.id == b.id && s.count == b.count && components_eq(s.components.as_ref(), b.components.as_ref())
}

/// Components equal up to key order.
fn components_eq(a: Option<&Nbt>, b: Option<&Nbt>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Nbt::Compound(x)), Some(Nbt::Compound(y))) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && v == v2))
        }
        _ => false,
    }
}

/// Ominous banner on the head and patrol leader.
pub fn is_captain(m: &Mob) -> bool {
    m.ill.patrol_leader && m.equipment[5].as_ref().is_some_and(is_ominous_banner)
}

// ---------- Goals ----------

fn hurt_by(alert: bool) -> Goal {
    Goal::HurtBy { base: TBase::new(true, false), alert, stamp: 0 }
}

fn nearest(who: Who, must_see: bool) -> Goal {
    super::surface::nearest(who, must_see, 10)
}

fn nearest_memory(who: Who, must_see: bool, memory: i32) -> Goal {
    let mut base = TBase::new(must_see, false);
    base.unseen_memory = memory;
    Goal::Nearest { base, who, interval: super::mth::reduced(10), target: None, angry_only: false, non_tame: false }
}

fn look_player(dist: f32, prob: f32) -> Goal {
    Goal::LookAtPlayer { dist, prob, at: None, time: 0 }
}

fn look_mob(dist: f32) -> Goal {
    Goal::Ill(IGoal::LookAtMob { dist, at: None, time: 0 })
}

fn stroll(speed: f64) -> Goal {
    Goal::Stroll { speed, interval: 120, prob: None, wanted: [0.0; 3] }
}

/// Common goals of raid pillagers (the "patrol monster" layer then the "raid pillager" layer), in the game's order.
fn raider_goals(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(4, Goal::Ill(IGoal::Patrol { cooldown_until: -1 }));
    g.add(1, Goal::Ill(IGoal::ObtainBanner { item: None, cache: Vec::new() }));
    g.add(3, Goal::Ill(IGoal::PathfindToRaid { recruit: 0 }));
    g.add(4, Goal::Ill(IGoal::MoveThroughVillage { poi: None, visited: Vec::new(), stuck: false }));
    g.add(5, Goal::Ill(IGoal::Celebrate));
}

/// Goals and targets in the game's insertion order.
pub fn register_goals(m: &mut Mob) {
    if m.kind != Kind::Vex {
        raider_goals(m);
    }
    let g = &mut m.goals;
    let t = &mut m.targets;
    match m.kind {
        Kind::Pillager => {
            g.add(0, Goal::Float);
            g.add(1, super::surface::avoid(Who::Exact(Kind::Creaking), 8.0, 1.0, 1.2));
            g.add(2, Goal::Ill(IGoal::HoldGround { radius2: 100.0 }));
            g.add(3, Goal::Ill(IGoal::Crossbow(goals::CrossbowState::default())));
            g.add(8, stroll(0.6));
            g.add(9, look_player(15.0, 1.0));
            g.add(10, look_mob(15.0));
            t.add(1, hurt_by(true));
            t.add(2, nearest(Who::Player, true));
            t.add(3, nearest(Who::Villager, false));
            t.add(3, nearest(Who::IronGolem, true));
        }
        Kind::Vindicator => {
            g.add(0, Goal::Float);
            g.add(1, super::surface::avoid(Who::Exact(Kind::Creaking), 8.0, 1.0, 1.2));
            g.add(2, Goal::Ill(IGoal::BreakDoor { door: None, time: 0, last: -1, zombie: false }));
            g.add(3, Goal::Ill(IGoal::OpenDoor { door: None }));
            g.add(4, Goal::Ill(IGoal::HoldGround { radius2: 100.0 }));
            g.add(5, super::surface::melee(1.0, false, super::goals::Melee::Plain));
            t.add(1, hurt_by(true));
            t.add(2, nearest(Who::Player, true));
            t.add(3, nearest(Who::Villager, true));
            t.add(3, nearest(Who::IronGolem, true));
            t.add(4, Goal::Ill(IGoal::Johnny { base: TBase::new(true, true), target: None }));
            g.add(8, stroll(0.6));
            g.add(9, look_player(3.0, 1.0));
            g.add(10, look_mob(8.0));
        }
        Kind::Evoker => {
            g.add(0, Goal::Float);
            g.add(1, Goal::Ill(IGoal::Casting));
            g.add(2, super::surface::avoid(Who::Player, 8.0, 0.6, 1.0));
            g.add(3, super::surface::avoid(Who::Exact(Kind::Creaking), 8.0, 0.6, 1.0));
            g.add(4, Goal::Ill(IGoal::spell(Spell::SummonVex)));
            g.add(5, Goal::Ill(IGoal::spell(Spell::Fangs)));
            g.add(6, Goal::Ill(IGoal::spell(Spell::Wololo)));
            g.add(8, stroll(0.6));
            g.add(9, look_player(3.0, 1.0));
            g.add(10, look_mob(8.0));
            t.add(1, hurt_by(true));
            t.add(2, nearest_memory(Who::Player, true, 300));
            t.add(3, nearest_memory(Who::Villager, false, 300));
            t.add(3, nearest(Who::IronGolem, false));
        }
        Kind::Illusioner => {
            g.add(0, Goal::Float);
            g.add(1, Goal::Ill(IGoal::Casting));
            g.add(3, super::surface::avoid(Who::Exact(Kind::Creaking), 8.0, 1.0, 1.2));
            g.add(4, Goal::Ill(IGoal::spell(Spell::Disappear)));
            g.add(5, Goal::Ill(IGoal::spell(Spell::Blindness)));
            g.add(
                6,
                Goal::Bow {
                    speed: 0.5,
                    min_interval: 20,
                    radius2: 225.0,
                    attack_time: -1,
                    see: 0,
                    cw: false,
                    back: false,
                    strafe: -1,
                },
            );
            g.add(8, stroll(0.6));
            g.add(9, look_player(3.0, 1.0));
            g.add(10, look_mob(8.0));
            t.add(1, hurt_by(true));
            t.add(2, nearest_memory(Who::Player, true, 300));
            t.add(3, nearest_memory(Who::Villager, false, 300));
            t.add(3, nearest_memory(Who::IronGolem, false, 300));
        }
        Kind::Vex => {
            g.add(0, Goal::Float);
            g.add(4, Goal::Ill(IGoal::VexCharge));
            g.add(8, Goal::Ill(IGoal::VexRandomMove));
            g.add(9, look_player(3.0, 1.0));
            g.add(10, look_mob(8.0));
            t.add(1, hurt_by(true));
            t.add(2, Goal::Ill(IGoal::VexCopyOwner { base: TBase::new(false, false) }));
            t.add(3, nearest(Who::Player, true));
        }
        Kind::Ravager => {
            g.add(0, Goal::Float);
            g.add(4, super::surface::melee(1.0, true, super::goals::Melee::Plain));
            g.add(5, Goal::Stroll { speed: 0.4, interval: 120, prob: Some(0.001), wanted: [0.0; 3] });
            g.add(6, look_player(6.0, 0.02));
            g.add(10, look_mob(8.0));
            t.add(2, hurt_by(true));
            t.add(3, nearest(Who::Player, true));
            t.add(4, Goal::Ill(IGoal::NearestAdultVillager { base: TBase::new(true, false), target: None }));
            t.add(4, nearest(Who::IronGolem, true));
        }
        Kind::Witch => {
            g.add(1, Goal::Float);
            g.add(2, Goal::Ill(IGoal::Ranged { attack_time: -1, see: 0 }));
            g.add(2, Goal::Stroll { speed: 1.0, interval: 120, prob: Some(0.001), wanted: [0.0; 3] });
            g.add(3, look_player(8.0, 0.02));
            g.add(3, super::surface::look_around());
            t.add(1, hurt_by(false));
            t.add(2, Goal::Ill(IGoal::HealRaider { base: TBase::new(true, false), target: None }));
            t.add(3, Goal::Ill(IGoal::WitchPlayers { base: TBase::new(true, false), target: None }));
        }
        _ => {}
    }
}

// ---------- Engine hooks ----------

/// Celebration sound.
pub fn celebrate_sound(k: Kind) -> &'static str {
    match k {
        Kind::Pillager => "minecraft:entity.pillager.celebrate",
        Kind::Vindicator => "minecraft:entity.vindicator.celebrate",
        Kind::Evoker => "minecraft:entity.evoker.celebrate",
        Kind::Illusioner => "minecraft:entity.illusioner.ambient",
        Kind::Ravager => "minecraft:entity.ravager.celebrate",
        _ => "minecraft:entity.witch.celebrate",
    }
}

/// The own possible-attack test: an illager does not attack a baby villager; illagers and the vex
/// linked to an evoker are allies of each other (no team).
pub fn can_attack(s: &Server, m: &Mob, target: Ent) -> bool {
    if !handles(m.kind) {
        return true;
    }
    let Ent::Mob(id) = target else { return true };
    let Some(o) = s.mob_ref(id) else { return true };
    if is_illager(m.kind) && matches!(o.kind, Kind::Villager | Kind::WanderingTrader) && o.is_baby() {
        return false;
    }
    !allied(s, m, o)
}

/// Allies: illagers among themselves; the evoker and its vexes (and those of its allies).
pub fn allied(s: &Server, m: &Mob, o: &Mob) -> bool {
    if is_illager(m.kind) && is_illager(o.kind) {
        return true;
    }
    if m.kind == Kind::Evoker && o.kind == Kind::Vex {
        let root = o.ill.owner_id.and_then(|id| s.mob_ref(id));
        return root.is_some_and(|r| r.id == m.id || is_illager(r.kind));
    }
    false
}

/// A raid pillager's hits do not trigger the
/// retaliation of the family's species.
pub fn ignores_attacker(s: &Server, m: &Mob, by: Ent) -> bool {
    if !handles(m.kind) {
        return false;
    }
    let Ent::Mob(id) = by else { return false };
    s.mob_ref(id).is_some_and(|o| is_raider(o.kind))
}

/// The ravager's immobility test (hit, stunned, roar).
pub fn immobile(m: &Mob) -> bool {
    m.kind == Kind::Ravager && (m.ill.attack_tick > 0 || m.ill.stun_tick > 0 || m.ill.roar_tick > 0)
}

/// The ravager's line-of-sight test: blind when stunned or roaring.
pub fn blind(m: &Mob) -> bool {
    m.kind == Kind::Ravager && (m.ill.stun_tick > 0 || m.ill.roar_tick > 0)
}

/// Inaction wear: the inaction wear of a pillager (+2 unconditional), vex (magic light
/// always 1).
pub fn always_idle_wear(k: Kind) -> bool {
    is_raider(k) || k == Kind::Vex
}

/// Vex: passes through blocks during its tick (entity without physics).
pub fn no_physics(m: &Mob) -> bool {
    m.kind == Kind::Vex && !m.removed
}

/// Patroller, raid pillager.
pub fn remove_when_far_away(m: &Mob, d2: f64) -> Option<bool> {
    if !handles(m.kind) || m.kind == Kind::Vex {
        return None;
    }
    if m.ill.raid.is_some() {
        return Some(false);
    }
    Some(!m.ill.patrolling || d2 > 16384.0)
}

/// The raid pillager's forced-persistence test.
pub fn custom_persistence(m: &Mob) -> bool {
    is_raider(m.kind) && m.ill.raid.is_some()
}

/// Damage bonus of the held weapon (`attack_damage` "add" modifiers of the right hand).
pub fn weapon_attack_bonus(s: &Server, m: &Mob) -> f64 {
    let Some(w) = m.equipment[0].as_ref() else { return 0.0 };
    let Some(v) = s.life.items.default_component(w, "minecraft:attribute_modifiers") else { return 0.0 };
    let list = v.as_array().cloned().unwrap_or_default();
    list.iter()
        .filter(|m| {
            m["type"].as_str() == Some("minecraft:attack_damage")
                && matches!(m["slot"].as_str().unwrap_or("any"), "mainhand" | "hand" | "any")
                && m["operation"].as_str().unwrap_or("add_value") == "add_value"
        })
        .filter_map(|m| m["amount"].as_f64())
        .sum()
}

impl Server {
    /// Equipment sent back to the client after a change.
    pub(crate) fn send_mob_equipment(&self, m: &Mob, slots: &[usize]) {
        if !m.track.seen {
            return;
        }
        let list: rc_proto_v777::entity::Equipment =
            slots.iter().map(|&i| (i as u8, m.equipment[i].as_ref().and_then(|s| self.net(Some(s))))).collect();
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Equipment { id: m.id, slots: list }));
    }

    /// Start of the AI step (witch: potions drunk; raid pillager: join a nearby raid).
    pub(crate) fn illager_pre_ai_step(&mut self, m: &mut Mob) {
        if !handles(m.kind) || !m.alive() {
            return;
        }
        if m.kind == Kind::Witch {
            self.witch_ai_step(m);
        }
        if is_raider(m.kind) {
            // Pillager read back with a raid: registered again (the game does it on read).
            if m.ill.raid.is_some() {
                self.raider_relink(m);
            }
            self.raider_ai_step(m);
        }
    }

    /// End of the AI step (ravager: speed, foliage, hit, stun, roar).
    pub(crate) fn illager_post_ai_step(&mut self, m: &mut Mob) {
        if m.kind == Kind::Ravager && m.alive() {
            self.ravager_ai_step(m);
        }
    }

    /// The species' own server AI step (spell casters: countdown; vindicator: doors in a raid).
    pub(crate) fn illager_custom_ai(&mut self, m: &mut Mob) {
        if m.kind == Kind::Vindicator && !m.no_ai {
            m.nav.can_open_doors = self.is_raided(m.block_pos());
        }
        if is_spellcaster(m.kind) && m.ill.spell_ticks > 0 {
            m.ill.spell_ticks -= 1;
        }
    }

    /// End of the tick (vex: limited life).
    pub(crate) fn illager_post_tick(&mut self, m: &mut Mob) {
        if m.kind == Kind::Vex {
            self.vex_post_tick(m);
        }
    }

    /// Before a melee hit (the ravager's).
    pub(crate) fn illager_before_attack(&mut self, m: &mut Mob) {
        if m.kind == Kind::Ravager {
            m.ill.attack_tick = 10;
            self.mob_event(m, 4);
            self.mob_sound(m, "minecraft:entity.ravager.attack", 1.0, 1.0);
        }
    }

    /// Own finalization, before the common spawn finalization (equipment, enchantments, patrol).
    pub(crate) fn illager_finalize_pre(&mut self, m: &mut Mob, reason: super::spawn::Reason) {
        if !handles(m.kind) {
            return;
        }
        let (_, special) = self.local_difficulty(m.block_pos());
        match m.kind {
            Kind::Pillager => {
                m.equipment[0] = Some(Stack { id: "minecraft:crossbow".into(), count: 1, components: None });
                // Weapon (25% × special), armor (none),
                // then the crossbow gets Piercing I one time in 300.
                self.spawn_weapon_enchant(m, special);
                if self.mobs.rng.next_int_bound(300) == 0
                    && let Some(s) = m.equipment[0].as_mut()
                {
                    crate::enchanting::upgrade(&self.life.items, s, "minecraft:piercing", 1);
                }
            }
            Kind::Vex => {
                m.equipment[0] = Some(Stack { id: "minecraft:iron_sword".into(), count: 1, components: None });
                m.drop_chance[0] = 0.0;
                self.spawn_weapon_enchant(m, special);
            }
            Kind::Illusioner => {
                m.equipment[0] = Some(Stack { id: "minecraft:bow".into(), count: 1, components: None });
            }
            _ => {}
        }
        if m.kind == Kind::Vex {
            return;
        }
        // The naturally born witch does not join raids.
        m.ill.can_join_raid = m.kind != Kind::Witch || reason != super::spawn::Reason::Natural;
        // The spawn finalization of patrol monsters.
        use super::spawn::Reason as R;
        let leader_roll = !matches!(reason, R::Patrol | R::Event | R::Structure);
        if leader_roll && self.mobs.rng.next_float() < 0.06 && can_be_leader(m.kind) {
            m.ill.patrol_leader = true;
        }
        if m.ill.patrol_leader {
            m.equipment[5] = Some(ominous_banner());
            m.drop_chance[5] = 2.0;
        }
        if reason == R::Patrol {
            m.ill.patrolling = true;
        }
    }

    /// Own finalization, after the common spawn finalization (vindicator: doors, axe).
    pub(crate) fn illager_finalize_post(&mut self, m: &mut Mob) {
        if m.kind == Kind::Vindicator {
            m.nav.can_open_doors = true;
            if m.ill.raid.is_none() {
                m.equipment[0] = Some(Stack { id: "minecraft:iron_axe".into(), count: 1, components: None });
            }
            let (_, special) = self.local_difficulty(m.block_pos());
            self.spawn_weapon_enchant(m, special);
        }
    }

    /// Weapon enchanted at spawn: 25% × special multiplier,
    /// provider `mob_spawn_equipment`; empty hand: no draw.
    fn spawn_weapon_enchant(&mut self, m: &mut Mob, special: f32) {
        let Some(s) = m.equipment[0].clone() else { return };
        if self.mobs.rng.next_float() < 0.25 * special {
            let s = super::spawn::mob_spawn_enchant(&self.life.pack, &self.life.items, s, special, &mut self.mobs.rng);
            m.equipment[0] = Some(s);
        }
    }

    /// The pillagers' AI step (before the rest): join a nearby raid, or stay active facing the
    /// player and the golem during its own.
    fn raider_ai_step(&mut self, m: &mut Mob) {
        if !m.ill.can_join_raid {
            return;
        }
        match m.ill.raid {
            None => {
                if self.time % 20 == 0 {
                    self.raider_try_join(m);
                }
            }
            Some(_) => {
                let t = m.target;
                let golem =
                    matches!(t, Some(Ent::Mob(id)) if self.mob_ref(id).is_some_and(|o| o.kind == Kind::IronGolem));
                if t == Some(Ent::Player) || golem {
                    m.no_action_time = 0;
                }
            }
        }
    }
}

/// Plain text of a name (text then children, translations left empty).
pub fn plain_name(n: &Nbt) -> String {
    match n {
        Nbt::String(s) => s.clone(),
        Nbt::List(l) => l.iter().map(plain_name).collect(),
        Nbt::Compound(_) => {
            let mut s = n.get("text").and_then(Nbt::as_str).unwrap_or("").to_owned();
            for e in nbtio::list(n.get("extra")) {
                s.push_str(&plain_name(e));
            }
            s
        }
        _ => String::new(),
    }
}

/// The "can be leader" test (neither ravager nor witch).
pub fn can_be_leader(k: Kind) -> bool {
    k != Kind::Ravager && k != Kind::Witch
}
