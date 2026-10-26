//! Family F01 (`server-mobs.md` § 4.1): variants of the base hostiles (husk, zombie
//! villager, stray, bogged, parched, cave spider, giant) and small pests
//! (silverfish, endermite, phantom). Own state, save, conversions (drowning, freezing,
//! cure, killed villager), effects of hits and arrows, spawn rules, goals.

use rc_proto::nbt::Nbt;

use super::effects::Effect;
use super::goals::{Goal, MOVE, Melee, TBase, Who};
use super::mob::{Attr, Ent, Mob};
use super::spawn::{Group, Reason};
use super::species::Kind;
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

/// Index of `minecraft:plains` in the villager type registry (game order).
pub const PLAINS_ID: i32 = 2;

/// Professions of the `villager_profession` registry (game registration order).
pub const PROFESSIONS: [&str; 15] = [
    "none",
    "armorer",
    "butcher",
    "cartographer",
    "cleric",
    "farmer",
    "fisherman",
    "fletcher",
    "leatherworker",
    "librarian",
    "mason",
    "nitwit",
    "shepherd",
    "toolsmith",
    "weaponsmith",
];

/// Villager types (game registration order).
pub const VILLAGER_TYPES: [&str; 7] = ["desert", "jungle", "plains", "savanna", "snow", "swamp", "taiga"];

/// State specific to the species of the family.
#[derive(Debug, Clone)]
pub struct Extra {
    /// Affliction time (zombie: eyes under water; skeleton: in powder snow).
    pub afflict: i32,
    /// Conversion in progress (drowning → drowned / zombie, freezing → stray): remaining ticks, −1 otherwise.
    pub conv_time: i32,
    /// Zombie villager: cure (remaining ticks, −1 otherwise), initiating player.
    pub cure_time: i32,
    pub cure_starter: Option<u128>,
    pub vdata_final: bool,
    pub villager_xp: i32,
    /// Endermite: age in ticks (`Lifetime`).
    pub life: i32,
    /// Phantom: size, anchor, aimed point, swoop phase, internal flight speed.
    pub size: i32,
    pub anchor: Option<[i32; 3]>,
    pub move_target: [f64; 3],
    pub swoop: bool,
    pub fly_speed: f32,
    /// Silverfish: countdown of the wake-up of kin.
    pub friends: i32,
    /// Active effects.
    pub effects: Vec<Effect>,
    /// Variant and sound variant re-read (identifiers), resolved in the registry by the
    /// server (`resolve_variants`).
    pub variant_names: [Option<String>; 2],
    /// Growth gained by feeding the baby (`ForcedAge`): age taken back to adult age;
    /// growth particle timer.
    pub forced_age: i32,
    pub forced_timer: i32,
    /// Entity flags re-read and rewritten: silent, custom name always visible, glowing set by
    /// command (no gravity: `surf.no_gravity`).
    pub silent: bool,
    pub name_visible: bool,
    pub glowing: bool,
    /// Wolf: wet, shaking off, shake progress (previous, current).
    pub wet: bool,
    pub shaking: bool,
    pub shake: [f32; 2],
}

impl Default for Extra {
    fn default() -> Extra {
        Extra {
            afflict: 0,
            conv_time: -1,
            cure_time: -1,
            cure_starter: None,
            vdata_final: false,
            villager_xp: 0,
            life: 0,
            size: 0,
            anchor: None,
            move_target: [0.0; 3],
            swoop: false,
            fly_speed: 0.1,
            friends: 0,
            effects: Vec::new(),
            variant_names: [None, None],
            forced_age: 0,
            forced_timer: 0,
            silent: false,
            name_visible: false,
            glowing: false,
            wet: false,
            shaking: false,
            shake: [0.0; 2],
        }
    }
}

/// Variant registry, default variant and sound variant registry
/// of the species whose variant is registered by identifier; default sound:
/// `classic`.
fn variant_regs(kind: Kind) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match kind {
        Kind::Pig => ("minecraft:pig_variant", "minecraft:temperate", "minecraft:pig_sound_variant"),
        Kind::Cow => ("minecraft:cow_variant", "minecraft:temperate", "minecraft:cow_sound_variant"),
        Kind::Chicken => ("minecraft:chicken_variant", "minecraft:temperate", "minecraft:chicken_sound_variant"),
        Kind::Wolf => ("minecraft:wolf_variant", "minecraft:pale", "minecraft:wolf_sound_variant"),
        Kind::Cat => ("minecraft:cat_variant", "minecraft:black", "minecraft:cat_sound_variant"),
        _ => return None,
    })
}

fn strip(s: &str) -> &str {
    s.trim_start_matches("minecraft:")
}

/// Own fields read from an entity.
pub fn load_extra(m: &mut Mob, e: &Nbt) {
    if m.kind.zombie_like() {
        let doors = int(e, "CanBreakDoors").is_some_and(|v| v != 0);
        super::brains::set_can_break_doors(m, doors);
    }
    let x = &mut m.x;
    let geti = |k: &str, d: i32| int(e, k).map_or(d, |v| v as i32);
    let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
    x.silent = flag("Silent");
    x.name_visible = flag("CustomNameVisible");
    x.glowing = flag("Glowing");
    m.surf.no_gravity = flag("NoGravity");
    if m.kind.ageable() {
        x.forced_age = geti("ForcedAge", 0);
    }
    if m.kind.zombie_like() {
        x.afflict = geti("InWaterTime", 0);
        x.conv_time = geti("DrownedConversionTime", -1);
    }
    if m.kind == Kind::Skeleton {
        x.afflict = geti("FreezingTime", 0);
        x.conv_time = geti("StrayConversionTime", -1);
    }
    // Villager data (zombie villager; villager too, for conversions).
    if matches!(m.kind, Kind::ZombieVillager | Kind::Villager) {
        let vd = e.get("VillagerData");
        if int(e, "VillagerDataFinalized").is_some_and(|v| v != 0) || vd.is_some() {
            x.vdata_final = true;
        }
        if let Some(vd) = vd {
            let ty = vd.get("type").and_then(Nbt::as_str).map(strip).unwrap_or("plains");
            let prof = vd.get("profession").and_then(Nbt::as_str).map(strip).unwrap_or("none");
            m.villager = [
                VILLAGER_TYPES.iter().position(|t| *t == ty).map_or(PLAINS_ID, |i| i as i32),
                PROFESSIONS.iter().position(|p| *p == prof).unwrap_or(0) as i32,
                vd.get("level").and_then(Nbt::as_i64).unwrap_or(1) as i32,
            ];
        }
        // Trade experience: follows the villager through its conversions.
        x.villager_xp = geti("Xp", 0);
    }
    if m.kind == Kind::ZombieVillager {
        x.cure_time = geti("ConversionTime", -1);
        x.cure_starter = if x.cure_time != -1 { nbtio::uuid(e.get("ConversionPlayer")) } else { None };
    }
    if m.kind == Kind::Endermite {
        x.life = geti("Lifetime", 0);
    }
    if m.kind == Kind::Phantom {
        x.size = geti("size", 0).clamp(0, 64);
        x.anchor = match e.get("anchor_pos") {
            Some(Nbt::IntArray(a)) if a.len() == 3 => Some([a[0], a[1], a[2]]),
            _ => None,
        };
    }
    if let Some(Nbt::List(l)) = e.get("active_effects") {
        x.effects = l.iter().filter_map(Effect::from_nbt).collect();
    }
    if variant_regs(m.kind).is_some() {
        // Identifier without namespace: `minecraft:` (identifier codec).
        let id = |k: &str| {
            e.get(k).and_then(Nbt::as_str).map(|s| if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") })
        };
        x.variant_names = [id("variant"), id("sound_variant")];
    }
}

/// Own fields written.
pub fn save_extra(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    let x = &m.x;
    // Written only if set (like the game).
    for (k, on) in [("CustomNameVisible", x.name_visible), ("Silent", x.silent), ("NoGravity", m.surf.no_gravity), ("Glowing", x.glowing)]
    {
        if on {
            set(e, k, Nbt::Byte(1));
        } else {
            nbtio::remove(e, k);
        }
    }
    if m.kind.zombie_like() {
        set(e, "CanBreakDoors", Nbt::Byte(i8::from(m.can_break_doors)));
        set(e, "DrownedConversionTime", Nbt::Int(if x.conv_time >= 0 { x.conv_time } else { -1 }));
        set(e, "InWaterTime", Nbt::Int(if m.eyes_in_water { x.afflict } else { -1 }));
    }
    if m.kind == Kind::Skeleton {
        set(e, "StrayConversionTime", Nbt::Int(if x.conv_time >= 0 { x.conv_time } else { -1 }));
        set(e, "FreezingTime", Nbt::Int(x.afflict));
    }
    if matches!(m.kind, Kind::ZombieVillager | Kind::Villager) {
        let [t, p, l] = m.villager;
        let vd = Nbt::Compound(vec![
            ("level".into(), Nbt::Int(l)),
            (
                "profession".into(),
                Nbt::String(format!("minecraft:{}", PROFESSIONS.get(p as usize).copied().unwrap_or("none"))),
            ),
            (
                "type".into(),
                Nbt::String(format!("minecraft:{}", VILLAGER_TYPES.get(t as usize).copied().unwrap_or("plains"))),
            ),
        ]);
        set(e, "VillagerData", vd);
        set(e, "Xp", Nbt::Int(x.villager_xp));
    }
    if m.kind == Kind::ZombieVillager {
        set(e, "VillagerDataFinalized", Nbt::Byte(i8::from(x.vdata_final)));
        set(e, "ConversionTime", Nbt::Int(if x.cure_time >= 0 { x.cure_time } else { -1 }));
        match x.cure_starter {
            Some(u) => set(e, "ConversionPlayer", nbtio::uuid_nbt(u)),
            None => nbtio::remove(e, "ConversionPlayer"),
        }
    }
    if m.kind == Kind::Endermite {
        set(e, "Lifetime", Nbt::Int(x.life));
    }
    if m.kind == Kind::Phantom {
        match x.anchor {
            Some(a) => set(e, "anchor_pos", Nbt::IntArray(a.to_vec())),
            None => nbtio::remove(e, "anchor_pos"),
        }
        set(e, "size", Nbt::Int(x.size));
    }
    if x.effects.is_empty() {
        nbtio::remove(e, "active_effects");
    } else {
        set(e, "active_effects", Nbt::List(x.effects.iter().map(Effect::to_nbt).collect()));
    }
}

/// Effect of arrows shot by the species: (effect, duration in ticks).
pub fn arrow_effect(kind: Kind) -> Option<(&'static str, i32)> {
    match kind {
        Kind::Stray => Some(("minecraft:slowness", 600)),
        Kind::Bogged => Some(("minecraft:poison", 100)),
        Kind::Parched => Some(("minecraft:weakness", 600)),
        _ => None,
    }
}

/// Own goals (`Goal::Extra`).
#[derive(Debug, Clone)]
pub enum Xg {
    /// Silverfish: waking of kin (counter in `Extra::friends`).
    SilverfishWake,
    /// Silverfish: wandering (interval 10) or merging into a host block.
    SilverfishMerge {
        merge: bool,
        dir: usize,
        stroll: Box<Goal>,
    },
    PhantomStrategy {
        next_sweep: i32,
    },
    PhantomSweep {
        scared: bool,
        cat_tick: i32,
    },
    PhantomCircle {
        angle: f32,
        dist: f32,
        height: f32,
        cw: f32,
    },
    PhantomTarget {
        next_scan: i32,
    },
}

impl Xg {
    pub fn flags(&self) -> u8 {
        match self {
            Xg::SilverfishWake | Xg::PhantomStrategy { .. } | Xg::PhantomTarget { .. } => 0,
            Xg::SilverfishMerge { .. } | Xg::PhantomSweep { .. } | Xg::PhantomCircle { .. } => MOVE,
        }
    }
}

fn melee() -> Goal {
    Goal::Melee {
        speed: 1.0,
        unseen: false,
        kind: Melee::Plain,
        path: None,
        pathed: [0.0; 3],
        recalc: 0,
        cd: 0,
        last: i64::MIN / 2,
    }
}

fn nearest_player() -> Goal {
    Goal::Nearest {
        base: TBase::new(true, false),
        who: Who::Player,
        interval: super::mth::reduced(10),
        target: None,
        angry_only: false,
        non_tame: false,
    }
}

/// Goals of species without a common base; true if the species is handled here.
pub fn register_goals(m: &mut Mob) -> bool {
    let g = &mut m.goals;
    let t = &mut m.targets;
    let stroll =
        |speed: f64, interval: i32, prob: Option<f32>| Goal::Stroll { speed, interval, prob, wanted: [0.0; 3] };
    match m.kind {
        // Giant: no goal.
        Kind::Giant => {}
        Kind::Silverfish => {
            g.add(1, Goal::Float);
            g.add(1, Goal::Surf(super::goals::surface_goals::SGoal::ClimbPowderSnow));
            g.add(3, Goal::Extra(Xg::SilverfishWake));
            g.add(4, melee());
            g.add(
                5,
                Goal::Extra(Xg::SilverfishMerge { merge: false, dir: 0, stroll: Box::new(stroll(1.0, 10, None)) }),
            );
            t.add(1, Goal::HurtBy { base: TBase::new(true, false), alert: true, stamp: 0 });
            t.add(2, nearest_player());
        }
        Kind::Endermite => {
            g.add(1, Goal::Float);
            g.add(1, Goal::Surf(super::goals::surface_goals::SGoal::ClimbPowderSnow));
            g.add(2, melee());
            g.add(3, stroll(1.0, 120, Some(0.001)));
            g.add(7, Goal::LookAtPlayer { dist: 8.0, prob: 0.02, at: None, time: 0 });
            g.add(8, Goal::LookAround { rel: [0.0; 2], time: 0 });
            t.add(1, Goal::HurtBy { base: TBase::new(true, false), alert: true, stamp: 0 });
            t.add(2, nearest_player());
        }
        Kind::Phantom => {
            g.add(1, Goal::Extra(Xg::PhantomStrategy { next_sweep: 0 }));
            g.add(2, Goal::Extra(Xg::PhantomSweep { scared: false, cat_tick: 0 }));
            g.add(3, Goal::Extra(Xg::PhantomCircle { angle: 0.0, dist: 0.0, height: 0.0, cw: 0.0 }));
            t.add(1, Goal::Extra(Xg::PhantomTarget { next_scan: super::mth::reduced(20) }));
        }
        _ => return false,
    }
    true
}

impl Server {
    /// Construction: navigation and attributes according to the species.
    pub(crate) fn variant_init(&self, m: &mut Mob) {
        if m.kind == Kind::CaveSpider {
            m.nav.climber = true;
        }
        if m.kind == Kind::Phantom {
            m.attrs.attack.base = f64::from(6 + m.x.size);
        }
        if let Some((reg, def, sound_reg)) = variant_regs(m.kind) {
            // Default variants of the constructor, then those already re-read (loaded mob).
            m.variant = self.variant_index(reg, def).unwrap_or(0);
            m.sound_variant = self.variant_index(sound_reg, "minecraft:classic").unwrap_or(0);
            // Default values of the synchronized data = constructor variants (otherwise a
            // mob of the first registry entry would never be announced).
            if let Some(slots) = super::mob::variant_slots(m.kind) {
                for ((i, ser), v) in slots.into_iter().zip([m.variant, m.sound_variant]) {
                    if let Some(d) = m.data.iter_mut().find(|d| d.index == i) {
                        d.default = rc_proto_v777::entity::Meta::Variant(v, ser);
                        d.value = d.default.clone();
                    }
                }
            }
        }
        self.resolve_variants(m);
        super::effects::apply_mob_modifiers(m);
    }

    fn variant_index(&self, reg: &str, name: &str) -> Option<i32> {
        self.regs.entries(reg).iter().position(|x| x == name).map(|i| i as i32)
    }

    /// Variant and sound variant re-read, translated into registry indices; an absent or
    /// unknown name keeps the current variant (like the game). Called by `load_mob`.
    fn resolve_variants(&self, m: &mut Mob) {
        let Some((reg, _, sound_reg)) = variant_regs(m.kind) else { return };
        let [v, s] = std::mem::take(&mut m.x.variant_names);
        if let Some(i) = v.and_then(|n| self.variant_index(reg, &n)) {
            m.variant = i;
        }
        if let Some(i) = s.and_then(|n| self.variant_index(sound_reg, &n)) {
            m.sound_variant = i;
        }
    }

    /// Re-reading of the NBT of an already built mob (complete):
    /// `Mob::load` then the variants, which need the registries. Single re-read point
    /// on the server side (generators, summoning, `data`).
    pub(crate) fn load_mob(&self, m: &mut Mob, e: &Nbt) {
        m.load(e);
        self.resolve_variants(m);
    }

    /// Complete NBT of a mob: `Mob::save` plus the variant identifiers, which
    /// need the registries.
    pub(crate) fn mob_nbt(&self, m: &Mob) -> Nbt {
        let mut e = m.save();
        if let Some((reg, _, sound_reg)) = variant_regs(m.kind) {
            for (key, r, i) in [("variant", reg, m.variant), ("sound_variant", sound_reg, m.sound_variant)] {
                if let Some(n) = self.regs.entries(r).get(i as usize) {
                    nbtio::set(&mut e, key, Nbt::String(n.clone()));
                }
            }
        }
        e
    }

    /// Villager type (identifier) of a zombie villager; the data become fixed.
    pub(crate) fn set_villager_type(&self, m: &mut Mob, t: i32) {
        let name = self
            .data
            .registry("minecraft:villager_type")
            .and_then(|r| r.entries.get(t as usize).cloned())
            .unwrap_or_default();
        m.villager[0] = VILLAGER_TYPES.iter().position(|v| *v == strip(&name)).map_or(t, |i| i as i32);
        m.x.vdata_final = true;
    }

    /// End of the spawn finalization of a husk: second pickup draw, mounted camel husk (10%).
    pub(crate) fn finalize_husk(&mut self, m: &mut Mob, reason: Reason, special: f32) {
        if reason != Reason::Conversion {
            m.can_pickup = self.mobs.rng.next_float() < 0.55 * special;
        }
        // The "tried" flag restarts from zero at each husk of the group (like the game).
        if reason != Reason::Natural {
            return;
        }
        let p = m.block_pos();
        let b = Mob::bbox_at([f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5], 1.7, 2.375);
        if !rc_physics::is_free(&self.world(), &b) {
            return;
        }
        if self.mobs.rng.next_float() < 0.1 {
            // Iron spear, mounted camel husk and parched at the rear (F04).
            m.equipment[0] = Some(Stack { id: "minecraft:iron_spear".into(), count: 1, components: None });
            self.husk_camel_jockey(m);
        }
    }

    /// Sky rule of the variants (standard monster already validated).
    pub(crate) fn variant_sky_rule(&self, kind: Kind, p: [i32; 3]) -> bool {
        match kind {
            Kind::Husk | Kind::Parched | Kind::CamelHusk => self.can_see_sky(p),
            Kind::Stray => {
                let mut c = p;
                loop {
                    c[1] += 1;
                    if self.bname(c) != "minecraft:powder_snow" {
                        break;
                    }
                }
                self.can_see_sky([c[0], c[1] - 1, c[2]])
            }
            _ => true,
        }
    }

    /// No player (spectator excluded) within `r` of the block center.
    pub(crate) fn no_player_within(&self, p: [i32; 3], r: f64) -> bool {
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.game_mode == 3 || super::mth::dist2(self.pos, c) >= r * r
    }

    /// Item dropped at a given height (with offset).
    pub(crate) fn drop_at_height(&mut self, m: &Mob, s: Stack, dy: f64) {
        let r = &mut self.mobs.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item([m.pos[0], m.pos[1] + dy, m.pos[2]], v, s, 10);
    }

    // ---------- Ticks ----------

    /// Before the common tick; true if the mob was converted (removed).
    pub(crate) fn variant_pre_tick(&mut self, m: &mut Mob) -> bool {
        match m.kind {
            Kind::Skeleton => {
                // Freezing in powder snow: 140 ticks then 300 before becoming a stray.
                let snow = self.in_powder_snow(m);
                if self.conversion_tracker(m, snow, 140) {
                    self.convert_drowned_like(m, Kind::Stray, 1048);
                    return true;
                }
            }
            Kind::ZombieVillager if m.alive() && m.x.cure_time >= 0 => {
                let amount = self.cure_progress(m);
                m.x.cure_time -= amount;
                if m.x.cure_time <= 0 {
                    self.finish_cure(m);
                    return true;
                }
            }
            Kind::Silverfish | Kind::Endermite => m.body_yaw = m.yaw,
            _ => {}
        }
        false
    }

    /// After the tick: drowning of zombies, lifetime of the endermite.
    pub(crate) fn variant_post_tick(&mut self, m: &mut Mob) {
        if m.removed {
            return;
        }
        match m.kind {
            Kind::Zombie | Kind::Husk => {
                let eyes = m.eyes_in_water;
                if self.conversion_tracker(m, eyes, 600) {
                    let (to, ev) = if m.kind == Kind::Husk { (Kind::Zombie, 1041) } else { (Kind::Zombie, 1040) };
                    // Zombie → drowned (family F08): species not simulated here, conversion pending.
                    match (m.kind, Kind::from_name("minecraft:drowned")) {
                        (Kind::Zombie, Some(d)) => self.convert_drowned_like(m, d, ev),
                        (Kind::Zombie, None) => {}
                        _ => self.convert_drowned_like(m, to, ev),
                    }
                }
            }
            Kind::Endermite => {
                if !m.persistent {
                    m.x.life += 1;
                }
                if m.x.life >= 2400 {
                    m.removed = true;
                }
            }
            _ => {}
        }
    }

    fn conversion_tracker(&self, m: &mut Mob, afflicted: bool, total: i32) -> bool {
        tracker_step(m, afflicted, total)
    }

    fn in_powder_snow(&self, m: &Mob) -> bool {
        let b = m.bbox();
        let lo = b.min.map(|v| (v + 1.0e-5).floor() as i32);
        let hi = b.max.map(|v| (v - 1.0e-5).floor() as i32);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    if self.bname([x, y, z]) == "minecraft:powder_snow" {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// New mob resulting from a "simple" conversion (position, velocity, equipment…).
    pub(crate) fn converted(&mut self, from: &mut Mob, to: Kind, keep_equipment: bool, keep_pickup: bool) -> Mob {
        let mut n = self.new_mob(to, from.pos, from.yaw);
        n.pitch = from.pitch;
        n.old = from.old;
        n.vel = from.vel;
        if keep_equipment {
            for i in 0..8 {
                if let Some(s) = from.equipment[i].take() {
                    n.equipment[i] = Some(s);
                    n.drop_chance[i] = from.drop_chance[i];
                }
            }
        }
        n.fall_distance = from.fall_distance;
        n.last_hurt_by_player = from.last_hurt_by_player;
        n.hurt_time = from.hurt_time;
        n.body_yaw = from.body_yaw;
        n.head_yaw = from.head_yaw;
        n.on_ground = from.on_ground;
        // Common: effects, baby, pickup, hand, AI, persistence, name.
        for e in &from.x.effects {
            super::effects::add(&mut n.x.effects, e.clone());
        }
        super::effects::apply_mob_modifiers(&mut n);
        if from.is_baby() {
            super::interact::set_baby(&mut n);
        }
        if n.kind.ageable() && from.kind.ageable() {
            n.age = from.age;
            n.x.forced_age = from.x.forced_age;
            n.x.forced_timer = from.x.forced_timer;
        }
        if keep_pickup {
            n.can_pickup = from.can_pickup;
        }
        n.left_handed = from.left_handed;
        n.no_ai = from.no_ai;
        n.persistent |= from.persistent;
        n.custom_name = from.custom_name.clone();
        n.damage_cooldown = from.damage_cooldown;
        // Flags, invulnerability; absorption, portal delay, tags and custom
        // data (kept in the compound read).
        n.x.name_visible = from.x.name_visible;
        n.x.silent = from.x.silent;
        n.surf.no_gravity = from.surf.no_gravity;
        n.invulnerable = from.invulnerable;
        if let Some(src) = &from.nbt {
            for key in ["AbsorptionAmount", "PortalCooldown", "Tags", "data"] {
                if let Some(v) = src.get(key) {
                    let dst = n.nbt.get_or_insert_with(|| Nbt::Compound(Vec::new()));
                    nbtio::set(dst, key, v.clone());
                }
            }
        }
        self.convert_mounts(from, &mut n);
        if from.kind.zombie_like() && n.kind.zombie_like() && from.can_break_doors {
            super::brains::set_can_break_doors(&mut n, true);
        }
        // Skeletons: the copied equipment re-evaluates the weapon (bow or melee), as at each
        // change of held item.
        if n.kind.skeleton_like() {
            super::brains::reassess_weapon(self, &mut n);
        }
        n
    }

    /// Conversion: the first passenger mounts the new form (the others dismount), which
    /// also takes the place of the old one on its vehicle (last).
    fn convert_mounts(&mut self, from: &mut Mob, n: &mut Mob) {
        let mut riders = std::mem::take(&mut from.mount.passengers).into_iter();
        from.mount.passive_riders.clear();
        if let Some(root) = riders.next() {
            match root {
                Ent::Player => {
                    if self.mobs.mounts.riding.vehicle == Some(from.id) {
                        self.mobs.mounts.riding.vehicle = Some(n.id);
                        n.mount.passengers.push(Ent::Player);
                    }
                }
                Ent::Mob(pid) => {
                    if let Some(p) = self.mobs.get_mut(pid) {
                        p.mount.vehicle = Some(n.id);
                        p.mount.boarding_cooldown = 0;
                        if p.kind.non_controlling_rider() {
                            n.mount.passive_riders.push(pid);
                        }
                        n.mount.passengers.push(root);
                    }
                }
            }
        }
        for r in riders {
            match r {
                Ent::Player => {
                    if self.mobs.mounts.riding.vehicle == Some(from.id) {
                        self.mobs.mounts.riding.vehicle = None;
                    }
                }
                Ent::Mob(pid) => {
                    if let Some(p) = self.mobs.get_mut(pid) {
                        p.mount.vehicle = None;
                    }
                }
            }
        }
        if let Some(vid) = from.mount.vehicle.take()
            && let Some(v) = self.mobs.get_mut(vid)
        {
            v.mount.passengers.retain(|e| *e != Ent::Mob(from.id));
            v.mount.passive_riders.retain(|&id| id != from.id);
            v.mount.passengers.push(Ent::Mob(n.id));
            if n.kind.non_controlling_rider() {
                v.mount.passive_riders.push(n.id);
            }
            n.mount.vehicle = Some(vid);
        }
    }

    /// Trade data (experience, offers, reputation) passed from one villager form to the other;
    /// offers and reputation only live in the compound read.
    pub(super) fn copy_trade_data(from: &Mob, to: &mut Mob) {
        to.x.villager_xp = from.x.villager_xp;
        // F05: live offers and gossip of the villager, then taken over by the new one.
        super::villager::copy_trade(from, to);
        let Some(src) = &from.nbt else { return };
        for key in ["Offers", "Gossips"] {
            if let Some(v) = src.get(key) {
                let dst = to.nbt.get_or_insert_with(|| Nbt::Compound(Vec::new()));
                nbtio::set(dst, key, v.clone());
            }
        }
    }

    /// Conversion by affliction (drowning, freezing): zombie attributes re-drawn, event.
    fn convert_drowned_like(&mut self, m: &mut Mob, to: Kind, event: i32) {
        let mut n = self.converted(m, to, true, true);
        if n.kind.zombie_like() {
            let (_, special) = self.local_difficulty(n.block_pos());
            zombie_attributes(&mut n, special, true);
        }
        let _ = self.send(rc_client::Event::LevelEvent { kind: event, pos: n.block_pos(), data: 0, global: false });
        self.add_mob(n);
        m.removed = true;
    }

    // ---------- Zombie villager ----------

    /// Start of the cure: weakness removed, strength for the duration, event 16.
    pub(crate) fn start_cure(&mut self, m: &mut Mob, starter: Option<u128>, time: i32) {
        m.x.cure_starter = starter;
        m.x.cure_time = time;
        self.remove_mob_effect(m, "minecraft:weakness");
        let amp = (self.level.difficulty_id() - 1).min(0);
        self.give_mob_effect(m, "minecraft:strength", time, amp);
        self.mob_event(m, 16);
    }

    /// Progress of the cure (nearby iron bars and beds).
    fn cure_progress(&self, m: &mut Mob) -> i32 {
        let mut amount = 1;
        if m.rng.next_float() < 0.01 {
            let mut count = 0;
            let (x0, y0, z0) = (m.pos[0] as i32, m.pos[1] as i32, m.pos[2] as i32);
            'outer: for x in x0 - 4..x0 + 4 {
                for y in y0 - 4..y0 + 4 {
                    for z in z0 - 4..z0 + 4 {
                        if count >= 14 {
                            break 'outer;
                        }
                        if self.btag_id([x, y, z], crate::tag_bits::TagId::SpeedsUpZombieVillagerCuring) {
                            if m.rng.next_float() < 0.3 {
                                amount += 1;
                            }
                            count += 1;
                        }
                    }
                }
            }
        }
        amount
    }

    /// End of the cure: becomes a villager (data kept, nausea, event 1027).
    fn finish_cure(&mut self, m: &mut Mob) {
        let mut v = self.converted(m, Kind::Villager, false, false);
        // Items with a guaranteed drop are dropped; those carrying a
        // curse of binding go into the villager's inventory, slot = index of the
        // equipment slot in its group (hand 0, off hand 1, feet 0…: two cursed items
        // of the same index overwrite each other, as in the game). The rest disappears with the zombie.
        const INV_INDEX: [usize; 8] = [0, 1, 0, 1, 2, 3, 0, 0];
        for (i, inv) in INV_INDEX.into_iter().enumerate() {
            let Some(s) = m.equipment[i].take() else { continue };
            if self.ench_has(&s, "minecraft:prevent_armor_change") {
                if let Some(slot) = v.npc.as_mut().and_then(|n| n.inv.get_mut(inv)) {
                    *slot = Some(s);
                }
            } else if m.drop_chance[i] > 1.0 {
                self.drop_at_height(m, s, 0.0);
            }
        }
        // Data and "fixed" flag taken over, then finalization: the type is only replaced by
        // that of the biome if the data were not fixed.
        v.x.vdata_final = m.x.vdata_final;
        v.villager = m.villager;
        Self::copy_trade_data(m, &mut v);
        let mut g = Group::default();
        self.finalize_spawn(&mut v, Reason::Conversion, &mut g);
        self.villager_cured(&mut v, m.x.cure_starter); // F05: reputation
        if m.x.cure_starter == Some(self.uuid) {
            // W8c: `cured_zombie_villager` (Zombie Doctor).
            use crate::progress::pred::Who;
            self.trig_cured(Who::Mob(m), Who::Mob(&v));
        }
        self.give_mob_effect(&mut v, "minecraft:nausea", 200, 0);
        let _ = self.send(rc_client::Event::LevelEvent { kind: 1027, pos: m.block_pos(), data: 0, global: false });
        self.add_mob(v);
        m.removed = true;
    }

    /// Villager killed by a zombie → zombie villager (Normal 50%,
    /// Hard 100%); true if converted (neither loot nor experience).
    pub(crate) fn killed_by_zombie(&mut self, v: &mut Mob, by: Option<Ent>) -> bool {
        if v.kind != Kind::Villager {
            return false;
        }
        let Some(Ent::Mob(kid)) = by else { return false };
        let Some((mut rng, kkind, id, kpos)) = self.mobs.killer.take() else { return false };
        let diff = self.level.difficulty_id();
        let mut converted = false;
        if id == kid && kkind.zombie_like() && (diff == 2 || diff == 3) && (diff == 3 || !rng.next_bool()) {
            let mut z = self.converted(v, Kind::ZombieVillager, true, true);
            let mut g = Group::default();
            z.x.vdata_final = v.x.vdata_final;
            self.finalize_spawn(&mut z, Reason::Conversion, &mut g);
            z.villager = v.villager;
            Self::copy_trade_data(v, &mut z);
            z.x.vdata_final = true;
            let at = kpos;
            let _ = self.send(rc_client::Event::LevelEvent { kind: 1026, pos: at, data: 0, global: false });
            self.add_mob(z);
            v.removed = true;
            converted = true;
        }
        self.mobs.killer = Some((rng, kkind, id, kpos));
        converted
    }

    // ---------- Hits ----------

    /// Effects of a successful hit of a mob (of the variants).
    pub(crate) fn on_mob_hit(&mut self, m: &mut Mob, t: Ent) {
        let empty_hand = m.equipment[0].is_none();
        if m.kind.zombie_like() {
            let (eff, _) = self.local_difficulty(m.block_pos());
            if empty_hand && m.on_fire() && m.rng.next_float() < eff * 0.3 {
                let ticks = 2 * eff as i32 * 20;
                match t {
                    Ent::Player => {
                        self.player_ignite(ticks);
                    }
                    Ent::Mob(id) => {
                        if let Some(i) = self.mob_index(id)
                            && let Some(o) = self.mobs.list[i].as_mut()
                            && o.fire < ticks
                        {
                            o.fire = ticks;
                        }
                    }
                }
            }
            if m.kind == Kind::Husk && empty_hand {
                let dur = 140 * eff as i32;
                self.effect_on(m, t, "minecraft:hunger", dur, 0);
            }
        }
        if m.kind == Kind::CaveSpider {
            let secs = match self.level.difficulty_id() {
                2 => 7,
                3 => 15,
                _ => 0,
            };
            if secs > 0 {
                self.effect_on(m, t, "minecraft:poison", secs * 20, 0);
            }
        }
    }

    /// Damage taken by a zombie on the server side, after damage dealt: on Hard, a chance (attribute
    /// `spawn_reinforcements`, entity's random draw) to call a kin of the same species, placed
    /// within 50 attempts at 7..40 blocks per axis (integer coordinates, like the game), who aims at the
    /// same target; each successful call lowers the chance of the caller and the called by 0.05.
    pub(crate) fn zombie_reinforcements(&mut self, m: &mut Mob, by: Option<Ent>) {
        let Some(chance) = m.attrs.reinforcements.as_ref().map(Attr::value) else { return };
        let Some(target) = m.target.or(by) else { return };
        if self.level.difficulty_id() != 3
            || f64::from(m.rng.next_float()) >= chance
            || !(self.rule("minecraft:spawn_mobs", true) && self.rule("minecraft:spawn_monsters", true))
        {
            return;
        }
        let b = m.block_pos();
        let mut n = self.new_mob(m.kind, [0.0; 3], 0.0);
        for _ in 0..50 {
            let mut off = || (7 + m.rng.next_int_bound(34)) * (m.rng.next_int_bound(3) - 1);
            let p = [b[0] + off(), b[1] + off(), b[2] + off()];
            if !self.spawn_placement_ok(n.kind, p) || !self.spawn_rules_for(n.kind, p, Reason::Reinforcement) {
                continue;
            }
            n.pos = p.map(f64::from);
            n.old = n.pos;
            let bb = n.bbox();
            // The no-obstacle test, caller included (taken out of the list during its hit).
            let entity_in_way = m.bbox().intersects(&bb) || !self.unobstructed(&bb, n.id);
            if self.player_near_alive(n.pos, 7.0)
                || entity_in_way
                || !rc_physics::is_free(&self.world(), &bb)
                || (n.kind != Kind::Drowned && self.box_has_liquid(&bb))
            {
                continue;
            }
            n.target = Some(target);
            let mut g = Group::default();
            self.finalize_spawn(&mut n, Reason::Reinforcement, &mut g);
            if let Some(r) = n.attrs.reinforcements.as_mut() {
                r.set_perm_mod("minecraft:reinforcement_callee_charge", f64::from(-0.05f32), 0);
            }
            self.add_mob(n);
            if let Some(r) = m.attrs.reinforcements.as_mut() {
                let had = r.mods.iter().find(|x| x.0 == "minecraft:reinforcement_caller_charge").map_or(0.0, |x| x.1);
                r.set_perm_mod("minecraft:reinforcement_caller_charge", had - 0.05, 0);
            }
            return;
        }
    }

    /// Effect applied by mob `m` (source) to its target.
    fn effect_on(&mut self, m: &Mob, t: Ent, id: &str, dur: i32, amp: i32) {
        match t {
            Ent::Player => {
                self.give_player_effect_from(id, dur, amp, Some(m));
            }
            Ent::Mob(mid) => {
                if let Some(i) = self.mob_index(mid) {
                    let mut o = self.mobs.list[i].take().expect("mob");
                    self.give_mob_effect(&mut o, id, dur, amp);
                    self.mobs.list[i] = Some(o);
                }
            }
        }
    }

    // ---------- Own goals ----------

    pub(crate) fn xg_can_use(&mut self, x: &mut Xg, m: &mut Mob) -> bool {
        match x {
            Xg::SilverfishWake => m.x.friends > 0,
            Xg::SilverfishMerge { merge, dir, stroll } => self.silverfish_merge_can_use(m, merge, dir, stroll),
            Xg::PhantomStrategy { .. } => self.phantom_can_attack(m),
            Xg::PhantomSweep { .. } => m.target.is_some() && m.x.swoop,
            Xg::PhantomCircle { .. } => m.target.is_none() || !m.x.swoop,
            Xg::PhantomTarget { next_scan } => self.phantom_target_can_use(m, next_scan),
        }
    }

    pub(crate) fn xg_can_continue(&mut self, x: &mut Xg, m: &mut Mob) -> bool {
        match x {
            Xg::SilverfishMerge { merge, stroll, .. } => !*merge && self.goal_can_continue(stroll, m),
            Xg::PhantomSweep { scared, cat_tick } => self.phantom_sweep_continue(m, scared, cat_tick),
            Xg::PhantomTarget { .. } => self.phantom_can_attack(m),
            _ => self.xg_can_use(x, m),
        }
    }

    pub(crate) fn xg_start(&mut self, x: &mut Xg, m: &mut Mob) {
        match x {
            Xg::SilverfishMerge { merge, dir, stroll } => {
                if *merge {
                    self.silverfish_merge(m, *dir);
                } else {
                    self.goal_start(stroll, m);
                }
            }
            Xg::PhantomStrategy { next_sweep } => {
                *next_sweep = super::mth::reduced(10);
                m.x.swoop = false;
                self.phantom_anchor_above_target(m);
            }
            Xg::PhantomCircle { angle, dist, height, cw } => {
                *dist = 5.0 + m.rng.next_float() * 10.0;
                *height = -4.0 + m.rng.next_float() * 9.0;
                *cw = if m.rng.next_bool() { 1.0 } else { -1.0 };
                self.phantom_select_next(m, angle, *dist, *height, *cw);
            }
            _ => {}
        }
    }

    pub(crate) fn xg_stop(&mut self, x: &mut Xg, m: &mut Mob) {
        match x {
            Xg::SilverfishMerge { merge, stroll, .. } if !*merge => self.goal_stop(stroll, m),
            Xg::PhantomStrategy { .. } => self.phantom_strategy_stop(m),
            Xg::PhantomSweep { .. } => {
                m.target = None;
                m.x.swoop = false;
            }
            _ => {}
        }
    }

    pub(crate) fn xg_tick(&mut self, x: &mut Xg, m: &mut Mob) {
        match x {
            Xg::SilverfishWake => self.silverfish_wake_tick(m),
            Xg::SilverfishMerge { merge, stroll, .. } if !*merge => self.goal_tick(stroll, m),
            Xg::PhantomStrategy { next_sweep } => self.phantom_strategy_tick(m, next_sweep),
            Xg::PhantomSweep { .. } => self.phantom_sweep_tick(m),
            Xg::PhantomCircle { angle, dist, height, cw } => self.phantom_circle_tick(m, angle, dist, height, cw),
            _ => {}
        }
    }
}

/// Follow-up of a mob conversion; true when the conversion must happen (300 ticks after
/// `total` ticks of affliction).
pub fn tracker_step(m: &mut Mob, afflicted: bool, total: i32) -> bool {
    if !m.alive() || m.no_ai {
        return false;
    }
    if afflicted {
        if m.x.conv_time >= 0 {
            m.x.conv_time -= 1;
            if m.x.conv_time < 0 {
                return true;
            }
        } else {
            m.x.afflict += 1;
            if m.x.afflict >= total {
                m.x.conv_time = 300;
            }
        }
    } else {
        // Outside affliction: counter at −1 (it will take `total` + 1 ticks next time).
        m.x.afflict = -1;
        m.x.conv_time = -1;
    }
    false
}

/// Setup of the zombie's attributes (entity's random draw); `conversion`: health not reset to maximum.
/// Reinforcement chance: base drawn (zombified piglin: 0 without a draw), leader bonus.
pub fn zombie_attributes(m: &mut Mob, special: f32, conversion: bool) {
    let base = if m.kind == Kind::ZombifiedPiglin { 0.0 } else { m.rng.next_double() * f64::from(0.1f32) };
    if let Some(r) = m.attrs.reinforcements.as_mut() {
        r.base = base;
        r.saved = true;
    }
    m.attrs.knockback_resistance.set_perm_mod(
        "minecraft:random_spawn_bonus",
        m.rng.next_double() * f64::from(0.05f32),
        0,
    );
    let fr = m.rng.next_double() * 1.5 * f64::from(special);
    if fr > 1.0 {
        m.attrs.follow.set_perm_mod("minecraft:zombie_random_spawn_bonus", fr, 2);
    }
    if m.rng.next_float() < special * 0.05 {
        let leader = m.rng.next_double() * 0.25 + 0.5;
        if let Some(r) = m.attrs.reinforcements.as_mut() {
            r.set_perm_mod("minecraft:leader_zombie_bonus", leader, 0);
        }
        let h = m.rng.next_double() * 3.0 + 1.0;
        m.attrs.max_health.set_perm_mod("minecraft:leader_zombie_bonus", h, 2);
        if !conversion {
            m.health = m.max_health();
        }
        super::brains::set_can_break_doors(m, true);
    }
}
