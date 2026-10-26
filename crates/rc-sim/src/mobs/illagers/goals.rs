//! Goals of family F07 (`Goal::Ill`): distant patrol, raid goals (banner, go
//! to the raid, cross the village, celebration), hold position, crossbow, spells, vex, look toward
//! a mob, own targets (Johnny, adult villager, healing allies, witch's players)
//! and potion throwing.

use super::super::goals::{LOOK, MOVE, TARGET, TBase};
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{block_of, dist2, reduced};
use super::super::species::Kind;
use super::{Spell, is_raider};
use crate::server::Server;

/// Crossbow goal state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrossbowState {
    /// 0 not loaded, 1 loading, 2 loaded, 3 ready to fire.
    pub state: u8,
    pub see: i32,
    pub attack_delay: i32,
    pub update_path_delay: i32,
}

#[derive(Debug, Clone)]
pub enum IGoal {
    /// Distant patrol (speeds 0.7 / leader 0.595).
    Patrol {
        cooldown_until: i64,
    },
    /// Pick up the ominous banner of a leaderless raid (targeted item entity).
    ObtainBanner {
        item: Option<i32>,
        cache: Vec<(i32, i64)>,
    },
    /// Join the raid center, recruit nearby pillagers every 20 ticks.
    PathfindToRaid {
        recruit: i32,
    },
    /// Cross the village from house to house (last 3 visited).
    MoveThroughVillage {
        poi: Option<[i32; 3]>,
        visited: Vec<[i32; 3]>,
        stuck: bool,
    },
    /// Zombie family:
    /// from occupied house to occupied house (last 16 visited), at night.
    VillageWalk {
        poi: Option<[i32; 3]>,
        visited: Vec<[i32; 3]>,
        path: Option<super::super::nav::Path>,
    },
    /// Celebration after a raid lost by the village.
    Celebrate,
    /// Patroller targeted by a target it has not yet attacked: calls its own.
    HoldGround {
        radius2: f32,
    },
    Crossbow(CrossbowState),
    /// Casting in progress.
    Casting,
    /// Spell: remaining casting delay, next allowed tick,
    /// last blinded target.
    UseSpell {
        spell: Spell,
        warmup: i32,
        next_tick: i32,
        last_target: i32,
    },
    /// Break down a door (240 ticks): vindicator in a raid (Normal or Hard, movement
    /// reserved) or zombie that has received the ability (`zombie`: Hard only, no
    /// flag, outside a raid).
    BreakDoor {
        door: Option<[i32; 3]>,
        time: i32,
        last: i32,
        zombie: bool,
    },
    OpenDoor {
        door: Option<[i32; 3]>,
    },
    /// "Johnny" vindicator: any attackable mob.
    Johnny {
        base: TBase,
        target: Option<Ent>,
    },
    VexCharge,
    VexRandomMove,
    VexCopyOwner {
        base: TBase,
    },
    /// Look toward the closest mob.
    LookAtMob {
        dist: f32,
        at: Option<Ent>,
        time: i32,
    },
    /// Ravager: adult villager.
    NearestAdultVillager {
        base: TBase,
        target: Option<Ent>,
    },
    /// Witch: potion throwing.
    Ranged {
        attack_time: i32,
        see: i32,
    },
    /// Witch in a raid: ally to heal (200-tick delay), players if healing is allowed.
    HealRaider {
        base: TBase,
        target: Option<Ent>,
    },
    WitchPlayers {
        base: TBase,
        target: Option<Ent>,
    },
}

impl IGoal {
    pub fn spell(spell: Spell) -> IGoal {
        IGoal::UseSpell { spell, warmup: 0, next_tick: 0, last_target: 0 }
    }

    pub fn village_walk() -> IGoal {
        IGoal::VillageWalk { poi: None, visited: Vec::new(), path: None }
    }

    pub fn flags(&self) -> u8 {
        match self {
            IGoal::Patrol { .. }
            | IGoal::VillageWalk { .. }
            | IGoal::ObtainBanner { .. }
            | IGoal::PathfindToRaid { .. }
            | IGoal::MoveThroughVillage { .. }
            | IGoal::Celebrate
            | IGoal::BreakDoor { zombie: false, .. }
            | IGoal::VexCharge
            | IGoal::VexRandomMove => MOVE,
            IGoal::HoldGround { .. } | IGoal::Crossbow(_) | IGoal::Casting | IGoal::Ranged { .. } => MOVE | LOOK,
            IGoal::LookAtMob { .. } => LOOK,
            IGoal::UseSpell { .. } | IGoal::OpenDoor { .. } | IGoal::BreakDoor { zombie: true, .. } => 0,
            IGoal::Johnny { .. }
            | IGoal::VexCopyOwner { .. }
            | IGoal::NearestAdultVillager { .. }
            | IGoal::HealRaider { .. }
            | IGoal::WitchPlayers { .. } => TARGET,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(
            self,
            IGoal::HoldGround { .. }
                | IGoal::Crossbow(_)
                | IGoal::VexCharge
                | IGoal::Ranged { .. }
                | IGoal::BreakDoor { .. }
                | IGoal::OpenDoor { .. }
        )
    }
}

/// Spell settings: casting, interval, delay (before reduction), preparation sound.
fn spell_params(s: Spell) -> (i32, i32, i32, &'static str) {
    match s {
        Spell::Fangs => (40, 100, 20, "minecraft:entity.evoker.prepare_attack"),
        Spell::SummonVex => (100, 340, 20, "minecraft:entity.evoker.prepare_summon"),
        Spell::Wololo => (60, 140, 40, "minecraft:entity.evoker.prepare_wololo"),
        Spell::Disappear => (20, 340, 20, "minecraft:entity.illusioner.prepare_mirror"),
        Spell::Blindness => (20, 180, 20, "minecraft:entity.illusioner.prepare_blindness"),
        Spell::None => (0, 0, 20, ""),
    }
}

/// Ground navigation.
pub(crate) fn ground_nav(m: &Mob) -> bool {
    !(m.nav.fly || m.nav.swim)
}

fn holds(m: &Mob, item: &str) -> bool {
    m.equipment[0].as_ref().is_some_and(|s| s.id == item) || m.equipment[1].as_ref().is_some_and(|s| s.id == item)
}

impl Server {
    // ---------- Dispatch ----------

    pub(crate) fn ill_can_use(&mut self, g: &mut IGoal, m: &mut Mob) -> bool {
        match g {
            IGoal::Patrol { cooldown_until } => {
                m.ill.patrolling
                    && m.target.is_none()
                    && m.mount.passengers.is_empty()
                    && m.ill.patrol_target.is_some()
                    && self.time >= *cooldown_until
            }
            IGoal::ObtainBanner { item, cache } => self.banner_can_use(m, item, cache),
            IGoal::PathfindToRaid { .. } => {
                m.target.is_none()
                    && m.mount.passengers.is_empty()
                    && self.raider_active_raid(m)
                    && !self.raider_raid_over(m)
                    && !self.is_village(m.block_pos())
            }
            IGoal::MoveThroughVillage { poi, visited, .. } => {
                if visited.len() > 2 {
                    visited.remove(0);
                }
                if !(self.raider_active_raid(m) && !self.raider_raid_over(m)) {
                    return false;
                }
                let Some(p) = self.random_home_poi(m, visited) else { return false };
                *poi = Some(p);
                m.target.is_none()
            }
            IGoal::VillageWalk { poi, visited, path } => self.village_walk_can_use(m, poi, visited, path),
            IGoal::Celebrate => m.alive() && m.target.is_none() && m.ill.raid.is_some_and(|id| self.raid_is_loss(id)),
            IGoal::HoldGround { .. } => {
                let player_hurt = matches!(m.last_hurt_by_mob, Some((Ent::Player, _)));
                m.ill.raid.is_none() && m.ill.patrolling && m.target.is_some() && !m.aggressive && !player_hurt
            }
            IGoal::Crossbow(_) => self.ent_valid(m.target) && holds(m, "minecraft:crossbow"),
            IGoal::Casting => m.ill.spell_ticks > 0,
            IGoal::UseSpell { spell, next_tick, last_target, .. } => {
                self.spell_can_use(m, *spell, *next_tick, *last_target)
            }
            IGoal::BreakDoor { door, zombie: true, .. } => {
                m.can_break_doors && ground_nav(m) && self.door_goal_can_use(m, door, Some(3))
            }
            IGoal::BreakDoor { door, .. } => {
                // The "active raid" test first: no draw outside a raid.
                if !self.raider_active_raid(m) || m.rng.next_int_bound(reduced(10)) != 0 {
                    return false;
                }
                self.door_goal_can_use(m, door, Some(2))
            }
            IGoal::OpenDoor { door } => self.door_goal_can_use(m, door, None) && self.raider_active_raid(m),
            IGoal::Johnny { base, target } => {
                if !m.ill.johnny {
                    return false;
                }
                *target = self.nearest_living(m, base.must_see, |_, _| true);
                target.is_some()
            }
            IGoal::VexCharge => {
                let Some(t) = m.target else { return false };
                if !self.ent_alive(t) || m.move_ctl.op == MoveOp::MoveTo {
                    return false;
                }
                if m.rng.next_int_bound(reduced(7)) != 0 {
                    return false;
                }
                self.ent_pos(t).is_some_and(|p| dist2(m.pos, p) > 4.0)
            }
            IGoal::VexRandomMove => m.move_ctl.op != MoveOp::MoveTo && m.rng.next_int_bound(reduced(7)) == 0,
            IGoal::VexCopyOwner { .. } => {
                let Some(o) = self.vex_owner(m) else { return false };
                let Some(t) = self.mob_ref(o).and_then(|o| o.target) else { return false };
                self.targeting_ok(m, t, false, -1.0, false, false)
            }
            IGoal::LookAtMob { dist, at, .. } => {
                if m.rng.next_float() >= 0.02 {
                    return false;
                }
                let d = *dist;
                *at = self.nearest_mob_in(m, d);
                at.is_some()
            }
            IGoal::NearestAdultVillager { base, target } => {
                if m.rng.next_int_bound(reduced(10)) != 0 {
                    return false;
                }
                let see = base.must_see;
                *target = self.nearest_living(m, see, |s, e| match e {
                    Ent::Mob(id) => s
                        .mob_ref(id)
                        .is_some_and(|o| matches!(o.kind, Kind::Villager | Kind::WanderingTrader) && !o.is_baby()),
                    Ent::Player => false,
                });
                target.is_some()
            }
            IGoal::Ranged { .. } => self.ent_valid(m.target),
            IGoal::HealRaider { base, target } => {
                if m.ill.heal_cooldown > 0 || !m.rng.next_bool() {
                    return false;
                }
                if !self.raider_active_raid(m) {
                    return false;
                }
                let see = base.must_see;
                *target = self.nearest_living(m, see, |s, e| match e {
                    Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| {
                        is_raider(o.kind) && o.kind != Kind::Witch && o.ill.raid.is_some_and(|r| s.raid_is_active(r))
                    }),
                    Ent::Player => false,
                });
                target.is_some()
            }
            IGoal::WitchPlayers { base, target } => {
                if !m.ill.attack_players || m.rng.next_int_bound(reduced(10)) != 0 {
                    return false;
                }
                let follow = m.follow_range();
                let see = base.must_see;
                *target = self.targeting_ok(m, Ent::Player, true, follow, see, true).then_some(Ent::Player);
                target.is_some()
            }
        }
    }

    pub(crate) fn ill_can_continue(&mut self, g: &mut IGoal, m: &mut Mob) -> bool {
        match g {
            IGoal::Patrol { .. } | IGoal::HoldGround { .. } | IGoal::Celebrate => self.ill_can_use(g, m),
            IGoal::ObtainBanner { item, .. } => {
                let Some(id) = *item else { return false };
                if !self.life.drops.iter().any(|d| d.id == id && !d.is_removed()) {
                    return false;
                }
                !m.nav.is_done() && !self.cannot_pick_up_banner(m)
            }
            IGoal::PathfindToRaid { .. } => {
                self.raider_active_raid(m) && !self.raider_raid_over(m) && !self.is_village(m.block_pos())
            }
            IGoal::VillageWalk { poi, .. } => {
                let Some(p) = *poi else { return false };
                !m.nav.is_done() && !closer_to_center(p, m.pos, f64::from(m.width()) + 4.0)
            }
            IGoal::MoveThroughVillage { poi, stuck, .. } => {
                if m.nav.is_done() {
                    return false;
                }
                let Some(p) = *poi else { return false };
                let r = f64::from(m.width()) + 1.0;
                m.target.is_none() && !closer_to_center(p, m.pos, r) && !*stuck
            }
            IGoal::Crossbow(_) => {
                self.ent_valid(m.target)
                    && (self.ill_can_use(g, m) || !m.nav.is_done())
                    && holds(m, "minecraft:crossbow")
            }
            IGoal::Casting => m.ill.spell_ticks > 0,
            IGoal::UseSpell { spell, warmup, .. } => {
                if *spell == Spell::Wololo {
                    return m.ill.wololo.is_some() && *warmup > 0;
                }
                self.ent_valid(m.target) && *warmup > 0
            }
            IGoal::BreakDoor { door, time, zombie, .. } => {
                let Some(d) = *door else { return false };
                let min_diff = if *zombie { 3 } else { 2 };
                (if *zombie { m.can_break_doors } else { self.raider_active_raid(m) })
                    && *time <= 240
                    && !self.door_is_open(d)
                    && closer_to_center(d, m.pos, 2.0)
                    && self.level.difficulty_id() >= min_diff
            }
            IGoal::OpenDoor { .. } => false,
            IGoal::Johnny { base, .. }
            | IGoal::VexCopyOwner { base }
            | IGoal::NearestAdultVillager { base, .. }
            | IGoal::HealRaider { base, .. }
            | IGoal::WitchPlayers { base, .. } => self.target_continue(base, m),
            IGoal::VexCharge => m.move_ctl.op == MoveOp::MoveTo && m.ill.vex_charging && self.ent_valid(m.target),
            IGoal::VexRandomMove => false,
            IGoal::LookAtMob { dist, at, time } => {
                let Some(e) = *at else { return false };
                if !self.ent_alive(e) {
                    return false;
                }
                let d = f64::from(*dist);
                self.ent_pos(e).is_some_and(|p| dist2(m.pos, p) <= d * d) && *time > 0
            }
            IGoal::Ranged { .. } => {
                self.ent_valid(m.target) || (m.target.is_some_and(|t| self.ent_alive(t)) && !m.nav.is_done())
            }
        }
    }

    pub(crate) fn ill_start(&mut self, g: &mut IGoal, m: &mut Mob) {
        match g {
            IGoal::ObtainBanner { item, .. } => {
                if let Some(p) = item.and_then(|id| self.life.drops.iter().find(|d| d.id == id)).map(|d| d.pos) {
                    let path = self.create_path(m, block_of(p), 1);
                    self.move_to_path(m, path, f64::from(1.15f32));
                }
            }
            IGoal::MoveThroughVillage { poi, stuck, .. } => {
                m.no_action_time = 0;
                if let Some(p) = *poi {
                    let t = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
                    self.move_to_pos(m, t, 1.05f32 as f64);
                }
                *stuck = false;
            }
            IGoal::VillageWalk { path, .. } => {
                let p = path.take();
                self.move_to_path(m, p, 1.0);
            }
            IGoal::Celebrate => m.ill.celebrating = true,
            IGoal::HoldGround { .. } => {
                m.nav.stop();
                let t = m.target;
                self.shout_to_raiders(m, t, false);
            }
            IGoal::Casting => m.nav.stop(),
            IGoal::UseSpell { spell, warmup, next_tick, last_target } => {
                let (casting, interval, warm, sound) = spell_params(*spell);
                *warmup = reduced(warm);
                m.ill.spell_ticks = casting;
                *next_tick = m.tick_count + interval;
                if !sound.is_empty() {
                    self.mob_sound(m, sound, 1.0, 1.0);
                }
                m.ill.spell = *spell;
                if *spell == Spell::Blindness
                    && let Some(Ent::Mob(id)) = m.target
                {
                    *last_target = id;
                } else if *spell == Spell::Blindness && m.target == Some(Ent::Player) {
                    *last_target = self.player_id;
                }
            }
            IGoal::BreakDoor { time, zombie, .. } => {
                *time = 0;
                // The vindicator also resets its inaction counter to zero.
                if !*zombie {
                    m.no_action_time = 0;
                }
            }
            // Open at start, closed again at stop (next tick).
            IGoal::OpenDoor { door } => {
                if let Some(d) = *door
                    && self.is_mob_door(d)
                {
                    self.set_door_open(d, true, m.id);
                }
            }
            IGoal::Johnny { base, target } => {
                m.no_action_time = 0;
                m.target = *target;
                Self::target_start(base);
            }
            IGoal::VexCopyOwner { base } => {
                m.target = self.vex_owner(m).and_then(|o| self.mob_ref(o)).and_then(|o| o.target);
                Self::target_start(base);
            }
            IGoal::HealRaider { base, target } => {
                m.ill.heal_cooldown = reduced(200);
                m.target = *target;
                Self::target_start(base);
            }
            IGoal::NearestAdultVillager { base, target } | IGoal::WitchPlayers { base, target } => {
                m.target = *target;
                Self::target_start(base);
            }
            IGoal::VexCharge => {
                if let Some(t) = m.target
                    && let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t))
                {
                    super::vex::set_wanted(m, [p[0], eye, p[2]], 1.0);
                }
                m.ill.vex_charging = true;
                self.mob_sound(m, "minecraft:entity.vex.charge", 1.0, 1.0);
            }
            IGoal::LookAtMob { time, .. } => *time = reduced(40 + m.rng.next_int_bound(40)),
            _ => {}
        }
    }

    pub(crate) fn ill_stop(&mut self, g: &mut IGoal, m: &mut Mob) {
        match g {
            IGoal::ObtainBanner { item, .. } => *item = None,
            IGoal::MoveThroughVillage { poi, visited, .. } => {
                if let Some(p) = *poi
                    && closer_to_center(p, m.pos, 1.0)
                {
                    visited.push(p);
                }
            }
            IGoal::VillageWalk { poi, visited, .. } => {
                if let Some(p) = *poi
                    && (m.nav.is_done() || closer_to_center(p, m.pos, 4.0))
                {
                    visited.push(p);
                }
            }
            IGoal::Celebrate => m.ill.celebrating = false,
            IGoal::HoldGround { .. } => {
                let t = m.target;
                if t.is_some() {
                    self.shout_to_raiders(m, t, true);
                    m.aggressive = true;
                }
            }
            IGoal::Crossbow(st) => {
                m.aggressive = false;
                m.target = None;
                st.see = 0;
                if m.use_ticks >= 0 {
                    m.use_ticks = -1;
                    m.ill.charging = false;
                    self.crossbow_unload(m);
                }
            }
            IGoal::Casting => m.ill.spell = Spell::None,
            IGoal::UseSpell { spell, .. } => {
                if *spell == Spell::Wololo {
                    m.ill.wololo = None;
                }
            }
            IGoal::BreakDoor { door: Some(d), .. } => self.door_break_progress(m.id, *d, -1),
            IGoal::OpenDoor { door: Some(d) } => {
                if self.is_mob_door(*d) {
                    self.set_door_open(*d, false, m.id);
                }
            }
            IGoal::Johnny { base, .. }
            | IGoal::VexCopyOwner { base }
            | IGoal::NearestAdultVillager { base, .. }
            | IGoal::HealRaider { base, .. }
            | IGoal::WitchPlayers { base, .. } => {
                m.target = None;
                base.target_mob = None;
            }
            IGoal::VexCharge => m.ill.vex_charging = false,
            IGoal::LookAtMob { at, .. } => *at = None,
            IGoal::Ranged { attack_time, see } => {
                *see = 0;
                *attack_time = -1;
            }
            _ => {}
        }
    }

    pub(crate) fn ill_tick(&mut self, g: &mut IGoal, m: &mut Mob) {
        match g {
            IGoal::Patrol { cooldown_until } => self.patrol_goal_tick(m, cooldown_until),
            IGoal::ObtainBanner { item, .. } => {
                let Some(id) = *item else { return };
                let Some(p) = self.life.drops.iter().find(|d| d.id == id && !d.is_removed()).map(|d| d.pos) else {
                    return;
                };
                if dist2(p, m.pos) < 1.414 * 1.414 {
                    self.raider_pick_up_banner(m, id);
                }
            }
            IGoal::PathfindToRaid { recruit } => self.pathfind_to_raid_tick(m, recruit),
            IGoal::MoveThroughVillage { poi, stuck, .. } => {
                if !m.nav.is_done() {
                    return;
                }
                let Some(p) = *poi else { return };
                let to = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
                let next = self
                    .pos_towards(m, 16, 7, to, std::f32::consts::PI / 10.0)
                    .or_else(|| self.pos_towards(m, 8, 7, to, std::f32::consts::PI / 2.0));
                match next {
                    Some(n) => {
                        self.move_to_pos(m, n, 1.05f32 as f64);
                    }
                    None => *stuck = true,
                }
            }
            IGoal::Celebrate => {
                if m.rng.next_int_bound(reduced(100)) == 0 {
                    let s = super::celebrate_sound(m.kind);
                    self.make_sound(m, s);
                }
                if m.mount.vehicle.is_none() && m.rng.next_int_bound(reduced(50)) == 0 {
                    m.jump_ctl = true;
                }
            }
            IGoal::HoldGround { radius2 } => {
                let Some(t) = m.target else { return };
                let Some(p) = self.ent_pos(t) else { return };
                if dist2(m.pos, p) > f64::from(*radius2) {
                    if let Some(eye) = self.ent_eye(t) {
                        self.look_at(m, [p[0], eye, p[2]], 30.0, 30.0);
                    }
                    if m.rng.next_int_bound(50) == 0
                        && let Some(s) = m.kind.info().ambient
                    {
                        self.make_sound(m, s);
                    }
                } else {
                    m.aggressive = true;
                }
            }
            IGoal::Crossbow(st) => self.crossbow_goal_tick(m, st),
            IGoal::Casting => {
                let at = m.target.or_else(|| m.ill.wololo.map(Ent::Mob));
                if let Some(t) = at
                    && let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t))
                {
                    let y = super::super::goals::max_head_y(m.kind);
                    self.look_at(m, [p[0], eye, p[2]], y, 40.0);
                }
            }
            IGoal::UseSpell { spell, warmup, .. } => {
                *warmup -= 1;
                if *warmup == 0 {
                    let s = *spell;
                    self.perform_spell(m, s);
                    let cast = if m.kind == Kind::Evoker {
                        "minecraft:entity.evoker.cast_spell"
                    } else {
                        "minecraft:entity.illusioner.cast_spell"
                    };
                    self.mob_sound(m, cast, 1.0, 1.0);
                }
            }
            IGoal::BreakDoor { door, time, last, zombie } => {
                let Some(d) = *door else { return };
                let min_diff = if *zombie { 3 } else { 2 };
                self.break_door_tick(m, d, time, last, min_diff);
            }
            IGoal::VexCharge => {
                let Some(t) = m.target else { return };
                let Some(tb) = self.ent_bbox(t) else { return };
                if m.touches(&tb) {
                    self.mob_attack(m, t);
                    m.ill.vex_charging = false;
                } else if let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t))
                    && dist2(m.pos, p) < 9.0
                {
                    super::vex::set_wanted(m, [p[0], eye, p[2]], 1.0);
                }
            }
            IGoal::VexRandomMove => self.vex_random_move(m),
            IGoal::LookAtMob { at, time, .. } => {
                let Some(e) = *at else { return };
                if self.ent_alive(e)
                    && let (Some(p), Some(eye)) = (self.ent_pos(e), self.ent_eye(e))
                {
                    self.look_at(m, [p[0], eye, p[2]], 10.0, 40.0);
                    *time -= 1;
                }
            }
            IGoal::Ranged { attack_time, see } => self.ranged_goal_tick(m, attack_time, see),
            _ => {}
        }
    }

    // ---------- Village crossing (zombie family) ----------

    /// At night, near a village, ground point (15/7) whose most "first" occupied house
    /// within 10 blocks (the game's search order) is the closest to the mob; path to it
    /// (doors according to the breaking ability, then the navigation keeps the doors opening,
    /// like the game), otherwise toward an intermediate step; cut at the first wooden door of
    /// the route.
    fn village_walk_can_use(
        &mut self,
        m: &mut Mob,
        poi: &mut Option<[i32; 3]>,
        visited: &mut Vec<[i32; 3]>,
        path: &mut Option<super::super::nav::Path>,
    ) -> bool {
        use crate::poi::Occupancy;
        if !ground_nav(m) {
            return false;
        }
        if visited.len() > 15 {
            visited.remove(0);
        }
        if self.bright_outside() {
            return false;
        }
        let pos = m.block_pos();
        if !self.close_to_village(pos, 6) {
            return false;
        }
        // Index of the points of interest around all the candidate points (±15 + 10 blocks;
        // distance to villages over ±7 sections), then read-only searches.
        let (cx, cz) = (pos[0] >> 4, pos[2] >> 4);
        for dz in -9..=9 {
            for dx in -9..=9 {
                self.poi_ensure(cx + dx, cz + dz);
            }
        }
        let homes: Vec<[i32; 3]> =
            self.poi_in_range(|t| t.is_village(), pos, 64, Occupancy::IsOccupied).into_iter().map(|r| r.pos).collect();
        let find = |c: [i32; 3], seen: &[[i32; 3]]| {
            homes.iter().copied().find(|h| {
                (h[0] - c[0]).abs() <= 10
                    && (h[2] - c[2]).abs() <= 10
                    && crate::poi::dist2i(*h, c) <= 100
                    && !seen.contains(h)
            })
        };
        let seen = visited.clone();
        let land = super::super::wild::rpos::land_pos(self, m, 15, 7, |s, _, p| {
            let sec = (p[0] >> 4, p[1] >> 4, p[2] >> 4);
            if s.mobs.poi.sections_to_village(sec) > 1 {
                return f64::NEG_INFINITY;
            }
            find(p, &seen).map_or(f64::NEG_INFINITY, |h| -(crate::poi::dist2i(h, pos) as f64))
        });
        let Some(land) = land else { return false };
        let Some(target) = find(block_of(land), visited) else { return false };
        *poi = Some(target);
        let doors = m.can_break_doors;
        m.nav.can_open_doors = doors;
        let mut p = self.create_path(m, target, 0);
        m.nav.can_open_doors = true;
        if p.is_none() {
            let to = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
            let Some(step) = self.pos_towards(m, 10, 7, to, std::f32::consts::FRAC_PI_2) else { return false };
            m.nav.can_open_doors = doors;
            p = self.create_path(m, block_of(step), 0);
            m.nav.can_open_doors = true;
            if p.is_none() {
                return false;
            }
        }
        let door_node =
            p.as_ref().and_then(|p| p.nodes.iter().map(|n| n.0).find(|n| self.wooden_door([n[0], n[1] + 1, n[2]])));
        if let Some(n) = door_node {
            p = self.create_path(m, n, 0);
        }
        *path = p;
        path.is_some()
    }

    // ---------- Tools ----------

    /// Living target.
    pub(crate) fn ent_valid(&self, t: Option<Ent>) -> bool {
        t.is_some_and(|t| self.ent_alive(t))
    }

    /// Closest mob (`Mob`) in the box (radius, 3, radius), sight required
    /// (non-combat targeting limited to range `d`), measured from the eyes.
    fn nearest_mob_in(&self, m: &mut Mob, d: f32) -> Option<Ent> {
        let d = f64::from(d);
        let b = m.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - d, b.min[1] - 3.0, b.min[2] - d],
            max: [b.max[0] + d, b.max[1] + 3.0, b.max[2] + d],
        };
        let eye = [m.pos[0], m.eye_y(), m.pos[2]];
        let cands: Vec<(i32, [f64; 3])> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.alive() && o.id != m.id && o.touches(&area))
            .map(|o| (o.id, o.pos))
            .collect();
        let mut best: Option<(f64, i32)> = None;
        for (id, p) in cands {
            if !self.targeting_ok(m, Ent::Mob(id), false, d, true, true) {
                continue;
            }
            let dd = dist2(eye, p);
            if best.is_none_or(|b| dd < b.0) {
                best = Some((dd, id));
            }
        }
        best.map(|b| Ent::Mob(b.1))
    }

    /// Closest living entity in the targeting zone (tracking, 4, tracking), combat conditions
    /// (the player included).
    fn nearest_living(&self, m: &mut Mob, see: bool, filter: impl Fn(&Server, Ent) -> bool) -> Option<Ent> {
        let f = m.follow_range();
        let b = m.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - f, b.min[1] - 4.0, b.min[2] - f],
            max: [b.max[0] + f, b.max[1] + 4.0, b.max[2] + f],
        };
        let eye = [m.pos[0], m.eye_y(), m.pos[2]];
        let mut cands: Vec<(Ent, [f64; 3])> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.alive() && o.id != m.id && o.touches(&area))
            .map(|o| (Ent::Mob(o.id), o.pos))
            .collect();
        if self.player_aabb().intersects(&area) {
            cands.push((Ent::Player, self.pos));
        }
        let mut best: Option<(f64, Ent)> = None;
        for (e, p) in cands {
            if !filter(self, e) || !self.targeting_ok(m, e, true, f, see, true) {
                continue;
            }
            let dd = dist2(eye, p);
            if best.is_none_or(|b| dd < b.0) {
                best = Some((dd, e));
            }
        }
        best.map(|b| b.1)
    }

    /// "Hold position" attack goal: pillagers within 8 blocks (sight ignored) take the target; at
    /// the end of the goal, they also become aggressive.
    fn shout_to_raiders(&mut self, m: &Mob, target: Option<Ent>, aggressive: bool) {
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 8.0), max: b.max.map(|v| v + 8.0) };
        for o in self.mobs.list.iter_mut().flatten() {
            if o.id != m.id && is_raider(o.kind) && o.alive() && o.touches(&area) && dist2(m.pos, o.pos) <= 64.0 {
                o.target = target;
                if aggressive {
                    o.aggressive = true;
                }
            }
        }
    }

    /// Tick of the distant patrol goal.
    fn patrol_goal_tick(&mut self, m: &mut Mob, cooldown_until: &mut i64) {
        let leader = m.ill.patrol_leader;
        if !m.nav.is_done() {
            return;
        }
        let companions = self.patrol_companions(m);
        let Some(target) = m.ill.patrol_target else { return };
        let tc = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
        if m.ill.patrolling && companions.is_empty() {
            m.ill.patrolling = false;
        } else if leader && closer_to_center(target, m.pos, 10.0) {
            self.find_patrol_target(m);
        } else {
            // Game quirk: the rotation is 90 RADIANS (tabulated sine and cosine).
            let d = [m.pos[0] - tc[0], m.pos[1] - tc[1], m.pos[2] - tc[2]];
            let (c, s) = (f64::from(super::super::mth::cos(90.0)), f64::from(super::super::mth::sin(90.0)));
            let r = [d[0] * c + d[2] * s, d[1], d[2] * c - d[0] * s];
            let ldt = [r[0] * 0.4 + tc[0], r[1] * 0.4 + tc[1], r[2] * 0.4 + tc[2]];
            let v = [ldt[0] - m.pos[0], ldt[1] - m.pos[1], ldt[2] - m.pos[2]];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let n = if l < f64::from(1.0e-5f32) { [0.0; 3] } else { v.map(|x| x / l) };
            let mt = [n[0] * 10.0 + m.pos[0], n[1] * 10.0 + m.pos[1], n[2] * 10.0 + m.pos[2]];
            let mut pt = block_of(mt);
            pt[1] = self.surface_y(pt[0], pt[2], 5);
            let speed = if leader { 0.595 } else { 0.7 };
            let ok = self.move_to_pos(m, pt.map(f64::from), speed);
            if !ok {
                self.patrol_move_randomly(m);
                *cooldown_until = self.time + 200;
            } else if leader {
                for id in companions {
                    if let Some(i) = self.mob_index(id)
                        && let Some(o) = self.mobs.list[i].as_mut()
                    {
                        o.ill.patrol_target = Some(pt);
                        o.ill.patrolling = true;
                    }
                }
            }
        }
    }

    /// Patrol companions: patrollers within 16 blocks (box), outside an active raid.
    fn patrol_companions(&self, m: &Mob) -> Vec<i32> {
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 16.0), max: b.max.map(|v| v + 16.0) };
        self.mobs
            .list
            .iter()
            .flatten()
            .filter(|o| {
                o.id != m.id
                    && is_raider(o.kind)
                    && o.touches(&area)
                    && !o.ill.raid.is_some_and(|r| self.raid_is_active(r))
            })
            .map(|o| o.id)
            .collect()
    }

    /// ±500 blocks around the mob (entity random).
    pub(crate) fn find_patrol_target(&self, m: &mut Mob) {
        let b = m.block_pos();
        let dx = -500 + m.rng.next_int_bound(1000);
        let dz = -500 + m.rng.next_int_bound(1000);
        m.ill.patrol_target = Some([b[0] + dx, b[1], b[2] + dz]);
        m.ill.patrolling = true;
    }

    /// ±8 blocks, heightmap height without foliage.
    fn patrol_move_randomly(&mut self, m: &mut Mob) -> bool {
        let b = m.block_pos();
        let x = b[0] - 8 + m.rng.next_int_bound(16);
        let z = b[2] - 8 + m.rng.next_int_bound(16);
        let y = self.surface_y(x, z, 5);
        self.move_to_pos(m, [f64::from(x), f64::from(y), f64::from(z)], 0.7)
    }

    /// Tick of the crossbow goal.
    fn crossbow_goal_tick(&mut self, m: &mut Mob, st: &mut CrossbowState) {
        let Some(t) = m.target else { return };
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        let has = self.line_of_sight(m, t);
        let had = st.see > 0;
        if has != had {
            st.see = 0;
        }
        if has {
            st.see += 1;
        } else {
            st.see -= 1;
        }
        let d = dist2(m.pos, tp);
        let needs_move = (d > 64.0 || st.see < 5) && st.attack_delay == 0;
        if needs_move {
            st.update_path_delay -= 1;
            if st.update_path_delay <= 0 {
                let speed = if st.state == 0 { 1.0 } else { 0.5 };
                self.move_to_entity(m, tp, speed);
                st.update_path_delay = 20 + m.rng.next_int_bound(21);
            }
        } else {
            st.update_path_delay = 0;
            m.nav.stop();
        }
        self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
        match st.state {
            0 => {
                if !needs_move {
                    m.use_ticks = 0;
                    st.state = 1;
                    m.ill.charging = true;
                }
            }
            1 => {
                if m.use_ticks < 0 {
                    st.state = 0;
                }
                let pull = m.use_ticks.max(0);
                if pull >= self.crossbow_charge_duration(m) {
                    // Last tick of use (loading), then stop.
                    self.crossbow_use_tick(m);
                    m.use_ticks = -1;
                    st.state = 2;
                    st.attack_delay = 20 + m.rng.next_int_bound(20);
                    m.ill.charging = false;
                }
            }
            2 => {
                st.attack_delay -= 1;
                if st.attack_delay == 0 {
                    st.state = 3;
                }
            }
            _ => {
                if has {
                    self.crossbow_shoot(m, t);
                    m.no_action_time = 0;
                    st.state = 0;
                }
            }
        }
    }

    /// Tick of the ranged attack objective (witch).
    fn ranged_goal_tick(&mut self, m: &mut Mob, attack_time: &mut i32, see: &mut i32) {
        let Some(t) = m.target else { return };
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        let d = dist2(m.pos, tp);
        let has = self.line_of_sight(m, t);
        if has {
            *see += 1;
        } else {
            *see = 0;
        }
        if d <= 100.0 && *see >= 5 {
            m.nav.stop();
        } else {
            self.move_to_entity(m, tp, 1.0);
        }
        self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
        *attack_time -= 1;
        if *attack_time == 0 {
            if !has {
                return;
            }
            let dist = (d.sqrt() as f32) / 10.0;
            let power = dist.clamp(0.1, 1.0);
            self.witch_throw(m, t, power);
            // Interval min = max = 60: `floor(dist × 0 + 60)`.
            *attack_time = 60;
        } else if *attack_time < 0 {
            *attack_time = 60;
        }
    }
}

/// Block center within `d`.
pub fn closer_to_center(b: [i32; 3], p: [f64; 3], d: f64) -> bool {
    let c = [f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5];
    dist2(c, p) < d * d
}
