//! Goals specific to simple surface animals (F02): leaving powder snow, going to
//! a block (garden, water, nesting beach), turtle goals, flying wandering, shoulder, following
//! another mob, polar bear targeting.

use super::super::mob::{Ent, Mob};
use super::super::mth::{dist2, reduced};
use super::super::species::Kind;
use super::super::surface;
use super::{JUMP, LOOK, MOVE, TARGET, TBase};
use crate::server::Server;

/// Block targeted by a "go to a block".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mtb {
    RaidGarden,
    TurtleWater,
    TurtleLayEgg,
    /// Trample turtle eggs (zombies, drowned): range 24, accepted at 1.14.
    RemoveEgg,
    /// Tamed cat: lie down on a block (bed, chest…; range 8, height −2..6).
    CatLie,
    /// Tamed cat: sit on a block (closed chest, lit furnace, bed foot side).
    CatSit,
}

#[derive(Debug, Clone)]
pub enum SGoal {
    ClimbPowderSnow,
    MoveToBlock {
        kind: Mtb,
        speed: f64,
        next_start: i32,
        try_ticks: i32,
        max_stay: i32,
        pos: [i32; 3],
        reached: bool,
        range: i32,
        vrange: i32,
        vstart: i32,
        want_raid: bool,
        can_raid: bool,
        /// Ticks spent on the target (egg trampling).
        eat_ticks: i32,
    },
    TurtlePanic {
        speed: f64,
        pos: [f64; 3],
    },
    TurtleGoHome {
        stuck: bool,
        close: i32,
    },
    TurtleTravel {
        stuck: bool,
    },
    TurtleStroll {
        wanted: [f64; 3],
    },
    ParrotWander {
        wanted: [f64; 3],
    },
    LandOnShoulder {
        on: bool,
    },
    FollowMob {
        following: Option<i32>,
        recalc: i32,
        old_water: f32,
    },
    PolarHurtBy {
        base: TBase,
        stamp: i32,
    },
    PolarAttackPlayers {
        base: TBase,
        target: Option<Ent>,
    },
}

impl SGoal {
    pub fn raid_garden() -> SGoal {
        SGoal::move_to_block(Mtb::RaidGarden, f64::from(0.7f32), 16, 1, 0)
    }

    pub fn move_to_block(kind: Mtb, speed: f64, range: i32, vrange: i32, vstart: i32) -> SGoal {
        SGoal::MoveToBlock {
            kind,
            speed,
            next_start: 0,
            try_ticks: 0,
            max_stay: 0,
            pos: [0; 3],
            reached: false,
            range,
            vrange,
            vstart,
            want_raid: false,
            can_raid: false,
            eat_ticks: 0,
        }
    }

    pub fn flags(&self) -> u8 {
        match self {
            SGoal::ClimbPowderSnow => JUMP,
            SGoal::MoveToBlock { .. } => MOVE | JUMP,
            SGoal::TurtlePanic { .. } | SGoal::TurtleStroll { .. } | SGoal::ParrotWander { .. } => MOVE,
            SGoal::TurtleGoHome { .. } | SGoal::TurtleTravel { .. } | SGoal::LandOnShoulder { .. } => 0,
            SGoal::FollowMob { .. } => MOVE | LOOK,
            SGoal::PolarHurtBy { .. } | SGoal::PolarAttackPlayers { .. } => TARGET,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, SGoal::ClimbPowderSnow | SGoal::MoveToBlock { .. })
    }
}

/// Centre of the block within `d`.
fn near_center(b: [i32; 3], p: [f64; 3], d: f64) -> bool {
    dist2([f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5], p) < d * d
}

impl Server {
    pub(super) fn surf_can_use(&mut self, g: &mut SGoal, m: &mut Mob) -> bool {
        match g {
            SGoal::ClimbPowderSnow => {
                // In powder snow (feet cell), species of the tag `powder_snow_walkable_mobs`.
                let p = m.block_pos();
                if self.bname(p) != "minecraft:powder_snow" {
                    return false;
                }
                let above = [p[0], p[1] + 1, p[2]];
                self.bname(above) == "minecraft:powder_snow" || !self.has_collision(above)
            }
            SGoal::MoveToBlock { kind, next_start, want_raid, can_raid, .. } => {
                let k = *kind;
                if k == Mtb::RemoveEgg {
                    // Removes the block: the search is done before re-arming the delay.
                    if !self.rule("minecraft:mob_griefing", true) {
                        return false;
                    }
                    if *next_start > 0 {
                        *next_start -= 1;
                        return false;
                    }
                    let found = self.mtb_find(g, m);
                    let delay = if found { reduced(20) } else { reduced(200 + m.rng.next_int_bound(200)) };
                    if let SGoal::MoveToBlock { next_start, .. } = g {
                        *next_start = delay;
                    }
                    return found;
                }
                if k == Mtb::RaidGarden && *next_start <= 0 {
                    if !self.rule("minecraft:mob_griefing", true) {
                        return false;
                    }
                    *can_raid = false;
                    *want_raid = m.surf.more_carrot <= 0;
                }
                match k {
                    Mtb::TurtleWater => {
                        let ok = if m.is_baby() && !m.in_water {
                            true
                        } else {
                            !m.surf.going_home && !m.in_water && !m.surf.has_egg
                        };
                        if !ok {
                            return false;
                        }
                    }
                    Mtb::TurtleLayEgg => {
                        if !(m.surf.has_egg && surface::turtle::home_within(m, 9.0)) {
                            return false;
                        }
                    }
                    // Cat conditions before the delay (the counter does not move otherwise).
                    Mtb::CatLie => {
                        if !m.tame || m.sitting || m.cat_lying {
                            return false;
                        }
                    }
                    Mtb::CatSit => {
                        if !m.tame || m.sitting {
                            return false;
                        }
                    }
                    Mtb::RaidGarden | Mtb::RemoveEgg => {}
                }
                if *next_start > 0 {
                    *next_start -= 1;
                    return false;
                }
                // The lying cat retries every 40 evaluations, without a draw.
                *next_start = if k == Mtb::CatLie { 40 } else { reduced(200 + m.rng.next_int_bound(200)) };
                self.mtb_find(g, m)
            }
            SGoal::TurtlePanic { pos, .. } => {
                if !self.should_panic(m, "minecraft:panic_causes") {
                    return false;
                }
                if let Some(w) = self.look_for_water(m, 7) {
                    *pos = [f64::from(w[0]), f64::from(w[1]), f64::from(w[2])];
                    return true;
                }
                match self.default_random_pos(m, 5, 4) {
                    Some(p) => {
                        *pos = p;
                        true
                    }
                    None => false,
                }
            }
            SGoal::TurtleGoHome { .. } => {
                if m.is_baby() {
                    return false;
                }
                if m.surf.has_egg {
                    return true;
                }
                if m.rng.next_int_bound(reduced(700)) != 0 {
                    return false;
                }
                !surface::turtle::home_within(m, 64.0)
            }
            SGoal::TurtleTravel { .. } => !m.surf.going_home && !m.surf.has_egg && m.in_water,
            SGoal::TurtleStroll { wanted } => {
                if m.in_water || m.surf.going_home || m.surf.has_egg {
                    return false;
                }
                if m.no_action_time >= 100 || m.rng.next_int_bound(reduced(100)) != 0 {
                    return false;
                }
                match self.default_random_pos(m, 10, 7) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            SGoal::ParrotWander { wanted } => {
                if m.no_action_time >= 100 || m.rng.next_int_bound(reduced(120)) != 0 {
                    return false;
                }
                match self.parrot_wander_pos(m) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            SGoal::LandOnShoulder { .. } => {
                let owner = m.tame && m.owner == Some(self.uuid) && self.ent_alive(Ent::Player);
                if !owner {
                    return false;
                }
                let can_sit_on = self.game_mode != 3
                    && !self.abilities().flying
                    && !self.player_in_water()
                    && !self.player_in_powder_snow();
                !m.sitting && can_sit_on && m.surf.ride_cd > 100
            }
            SGoal::FollowMob { following, .. } => {
                let bb = m.bbox();
                let area = rc_physics::Aabb { min: bb.min.map(|v| v - 7.0), max: bb.max.map(|v| v + 7.0) };
                *following = self
                    .mobs
                    .list
                    .iter()
                    .flatten()
                    .find(|o| {
                        o.kind != m.kind && o.id != m.id && o.alive() && o.touches(&area) && !o.invisible()
                    })
                    .map(|o| o.id);
                following.is_some()
            }
            SGoal::PolarHurtBy { base, stamp } => {
                let Some((by, ts)) = m.last_hurt_by_mob else { return false };
                if ts == *stamp {
                    return false;
                }
                if by == Ent::Player && self.rule("minecraft:universal_anger", false) {
                    return false;
                }
                self.target_can_attack(base, m, by, false, false)
            }
            SGoal::PolarAttackPlayers { target, .. } => {
                if m.is_baby() {
                    return false;
                }
                if m.rng.next_int_bound(reduced(20)) != 0 {
                    return false;
                }
                let follow = m.follow_range() * 0.5;
                let ok = self.player_attackable() && self.targeting_ok(m, Ent::Player, true, follow, true, true);
                *target = ok.then_some(Ent::Player);
                if target.is_none() {
                    return false;
                }
                let bb = m.bbox();
                let area = rc_physics::Aabb {
                    min: [bb.min[0] - 8.0, bb.min[1] - 4.0, bb.min[2] - 8.0],
                    max: [bb.max[0] + 8.0, bb.max[1] + 4.0, bb.max[2] + 8.0],
                };
                self.mobs.of_kind(Kind::PolarBear).any(|o| o.is_baby() && o.touches(&area))
            }
        }
    }

    pub(super) fn surf_can_continue(&mut self, g: &mut SGoal, m: &mut Mob) -> bool {
        match g {
            SGoal::MoveToBlock { kind, try_ticks, max_stay, pos, can_raid, .. } => {
                let (k, p) = (*kind, *pos);
                match k {
                    Mtb::TurtleWater => !m.in_water && *try_ticks <= 1200 && self.bname(p) == "minecraft:water",
                    _ => {
                        let base = *try_ticks >= -*max_stay && *try_ticks <= 1200;
                        if k == Mtb::RaidGarden {
                            // The valid target test read again while "can loot" is already true: false,
                            // the goal stops at the next full tick (game quirk reproduced).
                            *can_raid && base && self.mtb_valid(g, m, p)
                        } else if matches!(k, Mtb::RemoveEgg | Mtb::CatLie | Mtb::CatSit) {
                            base && self.mtb_valid(g, m, p)
                        } else {
                            base && self.mtb_valid(g, m, p) && m.surf.has_egg && surface::turtle::home_within(m, 9.0)
                        }
                    }
                }
            }
            SGoal::TurtlePanic { .. } | SGoal::TurtleStroll { .. } | SGoal::ParrotWander { .. } => !m.nav.is_done(),
            SGoal::TurtleGoHome { stuck, close } => {
                !surface::turtle::home_within(m, 7.0) && !*stuck && *close <= reduced(600)
            }
            SGoal::TurtleTravel { stuck } => {
                !m.nav.is_done() && !*stuck && !m.surf.going_home && !m.is_in_love() && !m.surf.has_egg
            }
            SGoal::FollowMob { following, .. } => {
                let Some(o) = following.and_then(|id| self.mob_ref(id)).map(|o| o.pos) else { return false };
                !m.nav.is_done() && dist2(m.pos, o) > 9.0
            }
            SGoal::PolarHurtBy { base, .. } => self.target_continue(base, m),
            SGoal::PolarAttackPlayers { base, .. } => self.polar_target_continue(base, m),
            _ => self.surf_can_use(g, m),
        }
    }

    pub(super) fn surf_start(&mut self, g: &mut SGoal, m: &mut Mob) {
        match g {
            SGoal::MoveToBlock { speed, pos, try_ticks, max_stay, eat_ticks, kind, .. } => {
                if matches!(kind, Mtb::CatLie | Mtb::CatSit) {
                    m.cat_sit_pose = false;
                }
                *eat_ticks = 0;
                let p = [f64::from(pos[0]) + 0.5, f64::from(pos[1] + 1), f64::from(pos[2]) + 0.5];
                let s = *speed;
                self.move_to_pos(m, p, s);
                *try_ticks = 0;
                let inner = m.rng.next_int_bound(1200) + 1200;
                *max_stay = m.rng.next_int_bound(inner) + 1200;
            }
            SGoal::TurtlePanic { speed, pos } => {
                let (s, p) = (*speed, *pos);
                self.move_to_pos(m, p, s);
            }
            SGoal::TurtleGoHome { stuck, close } => {
                m.surf.going_home = true;
                *stuck = false;
                *close = 0;
            }
            SGoal::TurtleTravel { stuck } => {
                let xt = m.rng.next_int_bound(1025) - 512;
                let mut yt = m.rng.next_int_bound(9) - 4;
                let zt = m.rng.next_int_bound(1025) - 512;
                if f64::from(yt) + m.pos[1] > f64::from(surface::sea_level(self) - 1) {
                    yt = 0;
                }
                m.surf.travel = Some(super::super::mth::block_of([
                    f64::from(xt) + m.pos[0],
                    f64::from(yt) + m.pos[1],
                    f64::from(zt) + m.pos[2],
                ]));
                *stuck = false;
            }
            SGoal::TurtleStroll { wanted } | SGoal::ParrotWander { wanted } => {
                let w = *wanted;
                self.move_to_pos(m, w, 1.0);
            }
            SGoal::LandOnShoulder { on } => *on = false,
            SGoal::FollowMob { recalc, old_water, .. } => {
                *recalc = 0;
                *old_water = super::super::nav::mob_malus(m, super::super::nav::PathType::Water);
                super::super::nav::set_malus(m, super::super::nav::PathType::Water, 0.0);
            }
            SGoal::PolarHurtBy { base, stamp } => {
                m.target = m.last_hurt_by_mob.map(|h| h.0);
                base.target_mob = m.target;
                *stamp = m.last_hurt_by_mob.map_or(0, |h| h.1);
                base.unseen_memory = 300;
                Self::target_start(base);
                if m.is_baby() {
                    self.polar_alert(m);
                    m.target = None;
                    base.target_mob = None;
                }
            }
            SGoal::PolarAttackPlayers { base, target } => {
                m.target = *target;
                Self::target_start(base);
            }
            SGoal::ClimbPowderSnow => {}
        }
    }

    pub(super) fn surf_stop(&mut self, g: &mut SGoal, m: &mut Mob) {
        match g {
            SGoal::TurtleGoHome { .. } => m.surf.going_home = false,
            SGoal::MoveToBlock { kind: Mtb::RemoveEgg, .. } => m.fall_distance = 1.0,
            SGoal::MoveToBlock { kind: Mtb::CatLie, .. } => m.cat_lying = false,
            SGoal::MoveToBlock { kind: Mtb::CatSit, .. } => m.cat_sit_pose = false,
            SGoal::TurtleTravel { .. } => m.surf.travel = None,
            SGoal::TurtleStroll { .. } | SGoal::ParrotWander { .. } => m.nav.stop(),
            SGoal::FollowMob { following, old_water, .. } => {
                *following = None;
                m.nav.stop();
                super::super::nav::set_malus(m, super::super::nav::PathType::Water, *old_water);
            }
            SGoal::PolarHurtBy { base, .. } | SGoal::PolarAttackPlayers { base, .. } => {
                m.target = None;
                base.target_mob = None;
            }
            _ => {}
        }
    }

    pub(super) fn surf_tick(&mut self, g: &mut SGoal, m: &mut Mob) {
        match g {
            SGoal::ClimbPowderSnow => m.jump_ctl = true,
            SGoal::MoveToBlock { .. } => self.mtb_tick(g, m),
            SGoal::TurtleGoHome { stuck, close } => {
                let home = m.surf.home;
                let close_to = surface::turtle::home_within(m, 16.0);
                if close_to {
                    *close += 1;
                }
                if m.nav.is_done() {
                    let hv = [f64::from(home[0]) + 0.5, f64::from(home[1]), f64::from(home[2]) + 0.5];
                    let mut next = self.pos_towards(m, 16, 3, hv, std::f32::consts::PI / 10.0);
                    if next.is_none() {
                        next = self.pos_towards(m, 8, 7, hv, std::f32::consts::FRAC_PI_2);
                    }
                    if let Some(n) = next
                        && !close_to
                        && self.bname(super::super::mth::block_of(n)) != "minecraft:water"
                    {
                        next = self.pos_towards(m, 16, 5, hv, std::f32::consts::FRAC_PI_2);
                    }
                    match next {
                        None => *stuck = true,
                        Some(n) => {
                            self.move_to_pos(m, n, 1.0);
                        }
                    }
                }
            }
            SGoal::TurtleTravel { stuck } => {
                let Some(t) = m.surf.travel else {
                    *stuck = true;
                    return;
                };
                if m.nav.is_done() {
                    let tv = [f64::from(t[0]) + 0.5, f64::from(t[1]), f64::from(t[2]) + 0.5];
                    let mut next = self.pos_towards(m, 16, 3, tv, std::f32::consts::PI / 10.0);
                    if next.is_none() {
                        next = self.pos_towards(m, 8, 7, tv, std::f32::consts::FRAC_PI_2);
                    }
                    if let Some(n) = next {
                        let (xc, zc) = (n[0].floor() as i32, n[2].floor() as i32);
                        let loaded = ((xc - 34) >> 4..=(xc + 34) >> 4).all(|cx| {
                            ((zc - 34) >> 4..=(zc + 34) >> 4)
                                .all(|cz| self.chunks.get(&(cx, cz)).is_some_and(Option::is_some))
                        });
                        if !loaded {
                            next = None;
                        }
                    }
                    match next {
                        None => *stuck = true,
                        Some(n) => {
                            self.move_to_pos(m, n, 1.0);
                        }
                    }
                }
            }
            SGoal::LandOnShoulder { on } => {
                if !*on && !m.sitting && m.touches(&self.player_aabb()) {
                    *on = surface::parrot::set_on_shoulder(self, m);
                }
            }
            SGoal::FollowMob { following, recalc, .. } => {
                let Some(o) = following.and_then(|id| self.mob_ref(id)).map(|o| (o.pos, o.eye_y(), o.look.wanted))
                else {
                    return;
                };
                self.look_at(m, [o.0[0], o.1, o.0[2]], 10.0, 40.0);
                *recalc -= 1;
                if *recalc <= 0 {
                    *recalc = reduced(10);
                    let d = dist2(m.pos, o.0);
                    if d > 9.0 {
                        self.move_to_entity(m, o.0, 1.0);
                    } else {
                        m.nav.stop();
                        // Squared distance compared with 3 (not 9), like in the game.
                        if d <= 3.0 || o.2 == m.pos {
                            let p = [m.pos[0] - (o.0[0] - m.pos[0]), m.pos[1], m.pos[2] - (o.0[2] - m.pos[2])];
                            self.move_to_pos(m, p, 1.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ---------- Go to a block ----------

    fn mtb_valid(&self, g: &mut SGoal, m: &Mob, p: [i32; 3]) -> bool {
        let SGoal::MoveToBlock { kind, want_raid, can_raid, .. } = g else { return false };
        match kind {
            Mtb::RaidGarden => {
                if *want_raid && !*can_raid && surface::rabbit::raid_target(self, p) {
                    *can_raid = true;
                    return true;
                }
                false
            }
            Mtb::TurtleWater => self.bname(p) == "minecraft:water",
            Mtb::RemoveEgg => {
                self.bname(p) == "minecraft:turtle_egg"
                    && self.is_air([p[0], p[1] + 1, p[2]])
                    && self.is_air([p[0], p[1] + 2, p[2]])
            }
            Mtb::TurtleLayEgg => {
                let _ = m;
                self.is_air([p[0], p[1] + 1, p[2]]) && self.btag_id(p, crate::tag_bits::TagId::Sand)
            }
            Mtb::CatLie => self.is_air([p[0], p[1] + 1, p[2]]) && self.btag_id(p, crate::tag_bits::TagId::CatsCanLieOn),
            Mtb::CatSit => {
                if !self.is_air([p[0], p[1] + 1, p[2]]) || !self.btag_id(p, crate::tag_bits::TagId::CatsCanSitOn) {
                    return false;
                }
                let name = self.bname(p);
                let st = self.st(p);
                if name == "minecraft:chest" {
                    self.life.openers.get(&p).copied().unwrap_or(0) < 1
                } else if name == "minecraft:furnace" {
                    self.data.state_properties(st).any(|kv| kv == ("lit", "true"))
                } else if self.btag_id(p, crate::tag_bits::TagId::Beds) {
                    !self.data.state_properties(st).any(|kv| kv == ("part", "head"))
                } else {
                    true
                }
            }
        }
    }

    /// The search for the nearest block (game traversal order).
    fn mtb_find(&self, g: &mut SGoal, m: &Mob) -> bool {
        let SGoal::MoveToBlock { range, vrange, vstart, .. } = g else { return false };
        let (h, v) = (*range, *vrange);
        let mut y = *vstart;
        let b = m.block_pos();
        while y <= v {
            for r in 0..h {
                let mut x = 0;
                while x <= r {
                    let mut z = if x < r && x > -r { r } else { 0 };
                    while z <= r {
                        let p = [b[0] + x, b[1] + y - 1, b[2] + z];
                        if self.mtb_valid(g, m, p) {
                            if let SGoal::MoveToBlock { pos, .. } = g {
                                *pos = p;
                            }
                            return true;
                        }
                        z = if z > 0 { -z } else { 1 - z };
                    }
                    x = if x > 0 { -x } else { 1 - x };
                }
            }
            y = if y > 0 { -y } else { 1 - y };
        }
        false
    }

    fn mtb_tick(&mut self, g: &mut SGoal, m: &mut Mob) {
        let SGoal::MoveToBlock { kind, speed, try_ticks, pos, reached, next_start, can_raid, eat_ticks, .. } = g else {
            return;
        };
        let target = [pos[0], pos[1] + 1, pos[2]];
        let accepted = if *kind == Mtb::RemoveEgg { 1.14 } else { 1.0 };
        if !near_center(target, m.pos, accepted) {
            *reached = false;
            *try_ticks += 1;
            let every = if *kind == Mtb::TurtleWater { 160 } else { 40 };
            if *try_ticks % every == 0 {
                let p = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
                let s = *speed;
                self.move_to_pos(m, p, s);
            }
        } else {
            *reached = true;
            *try_ticks -= 1;
        }
        match kind {
            Mtb::RaidGarden => {
                let p = *pos;
                self.look_at(m, [f64::from(p[0]) + 0.5, f64::from(p[1] + 1), f64::from(p[2]) + 0.5], 10.0, 40.0);
                if *reached {
                    if *can_raid {
                        surface::rabbit::eat_carrot(self, m, p);
                    }
                    *can_raid = false;
                    *next_start = 10;
                }
            }
            Mtb::TurtleLayEgg => {
                if !m.in_water && *reached {
                    if m.surf.lay_counter < 1 {
                        m.surf.laying = true;
                        m.surf.lay_counter = 1;
                    } else if m.surf.lay_counter > 200 {
                        let p = *pos;
                        surface::turtle::lay_egg(self, m, p);
                    }
                    if m.surf.laying {
                        m.surf.lay_counter += 1;
                    }
                }
            }
            Mtb::RemoveEgg => {
                let (p, r, e) = (*pos, *reached, *eat_ticks);
                *eat_ticks = self.egg_eat(m, p, r, e);
            }
            Mtb::TurtleWater => {}
            Mtb::CatLie => {
                m.cat_sit_pose = false;
                m.cat_lying = *reached;
            }
            Mtb::CatSit => m.cat_sit_pose = *reached,
        }
    }

    /// Body of the "trample eggs" goal: jumps in place, sounds, block removal after 60 ticks.
    /// Returns the new counter of ticks spent on the target.
    pub(crate) fn egg_eat(&mut self, m: &mut Mob, goal_pos: [i32; 3], reached: bool, ticks: i32) -> i32 {
        let mp = m.block_pos();
        let near = |o: [i32; 3]| [mp[0] + o[0], mp[1] + o[1], mp[2] + o[2]];
        let eat_pos = [[0, 0, 0], [0, -1, 0], [-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1], [0, -2, 0]]
            .into_iter()
            .map(near)
            .find(|&p| self.bname(p) == "minecraft:turtle_egg");
        let (true, Some(ep)) = (reached, eat_pos) else { return ticks };
        if ticks > 0 {
            m.vel[1] = 0.3;
            // Three egg crumbs above the block (spread drawn on the entity's random).
            let mut d = [0.0f32; 3];
            for v in &mut d {
                *v = (m.rng.next_float() - 0.5) * 0.08;
            }
            let c = [f64::from(ep[0]) + 0.5, f64::from(ep[1]) + 0.7, f64::from(ep[2]) + 0.5];
            self.item_particles("minecraft:egg", c, d, 0.15, 3);
        }
        if ticks % 2 == 0 {
            m.vel[1] = -0.3;
            if ticks % 6 == 0 {
                let pitch = 0.9 + m.rng.next_float() * 0.2;
                let c = [f64::from(goal_pos[0]) + 0.5, f64::from(goal_pos[1]) + 0.5, f64::from(goal_pos[2]) + 0.5];
                self.world_sound("minecraft:entity.zombie.destroy_egg", 5, c, 0.5, pitch);
            }
        }
        if ticks > 60 {
            self.remove_block(ep, false);
            // Twenty puffs, one per send, Gaussian spread of the entity's random.
            let c = [f64::from(ep[0]) + 0.5, f64::from(ep[1]), f64::from(ep[2]) + 0.5];
            for _ in 0..20 {
                let d = [0; 3].map(|_| (m.rng.next_gaussian() * 0.02) as f32);
                self.particles("minecraft:poof", c, d, 0.15, 1);
            }
            let pitch = 0.9 + self.mobs.rng.next_float() * 0.2;
            let c = [f64::from(ep[0]) + 0.5, f64::from(ep[1]) + 0.5, f64::from(ep[2]) + 0.5];
            self.world_sound("minecraft:entity.turtle.egg_break", 4, c, 0.7, pitch);
        }
        ticks + 1
    }

    // ---------- Random positions ----------

    /// The direction draw in a cone.
    fn dir_within(m: &mut Mob, h: f64, v: i32, fly_h: i32, xd: f64, zd: f64, max: f64) -> Option<[i32; 3]> {
        let center = super::super::mth::atan2(zd, xd) - f64::from(std::f32::consts::FRAC_PI_2);
        let yr = center + f64::from(2.0 * m.rng.next_float() - 1.0) * max;
        let dist = m.rng.next_double().sqrt() * h * f64::from(std::f32::consts::SQRT_2);
        let xt = -dist * yr.sin();
        let zt = dist * yr.cos();
        if xt.abs() > h || zt.abs() > h {
            return None;
        }
        let yt = m.rng.next_int_bound(2 * v + 1) - v + fly_h;
        Some([xt.floor() as i32, yt, zt.floor() as i32])
    }

    fn toward_dir(m: &Mob, d: [i32; 3]) -> [i32; 3] {
        super::super::mth::block_of([
            f64::from(d[0]) + m.pos[0],
            f64::from(d[1]) + m.pos[1],
            f64::from(d[2]) + m.pos[2],
        ])
    }

    fn best_of(&self, m: &mut Mob, f: &mut dyn FnMut(&Self, &mut Mob) -> Option<[i32; 3]>) -> Option<[f64; 3]> {
        let mut best: Option<(f32, [i32; 3])> = None;
        for _ in 0..10 {
            if let Some(p) = f(self, m) {
                let w = self.walk_target_value(m, p);
                if w > best.map_or(f32::NEG_INFINITY, |b| b.0) {
                    best = Some((w, p));
                }
            }
        }
        best.map(|b| [f64::from(b.1[0]) + 0.5, f64::from(b.1[1]), f64::from(b.1[2]) + 0.5])
    }

    /// The position draw towards a point.
    pub(crate) fn pos_towards(&self, m: &mut Mob, h: i32, v: i32, to: [f64; 3], max: f32) -> Option<[f64; 3]> {
        let (xd, zd) = (to[0] - m.pos[0], to[2] - m.pos[2]);
        self.best_of(m, &mut |s, m| {
            let d = Self::dir_within(m, f64::from(h), v, 0, xd, zd, f64::from(max))?;
            let p = Self::toward_dir(m, d);
            (!s.outside_limits(p) && s.nav_stable(m, p) && !s.has_malus(m, p)).then_some(p)
        })
    }

    /// Wander position of the parrot (nearby trees, flyover, air and water).
    fn parrot_wander_pos(&self, m: &mut Mob) -> Option<[f64; 3]> {
        let mut pos = None;
        if m.in_water {
            pos = self.land_random_pos(m, 15, 15);
        }
        if m.rng.next_float() >= 0.001 {
            pos = self.tree_pos(m);
        }
        if pos.is_some() {
            return pos;
        }
        self.fly_wander_pos(m)
    }

    /// Hovering position in front, otherwise
    /// air or water (F16: wither; the parrot falls back to it).
    pub(crate) fn fly_wander_pos(&self, m: &mut Mob) -> Option<[f64; 3]> {
        let (yr, pr) = (-m.yaw * super::super::mth::DEG, m.pitch * super::super::mth::DEG);
        let view = [
            f64::from(super::super::mth::sin(yr) * super::super::mth::cos(pr)),
            f64::from(super::super::mth::cos(yr) * super::super::mth::cos(pr)),
        ];
        let hover = self.best_of(m, &mut |s, m| {
            let d = Self::dir_within(m, 8.0, 7, 0, view[0], view[1], f64::from(std::f32::consts::FRAC_PI_2))?;
            let p = Self::toward_dir(m, d);
            if s.outside_limits(p) || !s.nav_stable(m, p) {
                return None;
            }
            let above = m.rng.next_int_bound(3) + 1;
            let p = s.move_up_to_above_solid(p, above);
            (s.mob_fluid(p) != super::super::env::Fluid::Water && !s.has_malus(m, p)).then_some(p)
        });
        if hover.is_some() {
            return hover;
        }
        self.best_of(m, &mut |s, m| {
            let d = Self::dir_within(m, 8.0, 4, -2, view[0], view[1], f64::from(std::f32::consts::FRAC_PI_2))?;
            let p = Self::toward_dir(m, d);
            if s.outside_limits(p) {
                return None;
            }
            let p = s.move_up_out_of_solid(p);
            (!s.has_malus(m, p)).then_some(p)
        })
    }

    fn tree_pos(&self, m: &Mob) -> Option<[f64; 3]> {
        let mp = m.block_pos();
        let lo = [(m.pos[0] - 3.0).floor() as i32, (m.pos[1] - 6.0).floor() as i32, (m.pos[2] - 3.0).floor() as i32];
        let hi = [(m.pos[0] + 3.0).floor() as i32, (m.pos[1] + 6.0).floor() as i32, (m.pos[2] + 3.0).floor() as i32];
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let p = [x, y, z];
                    if p == mp {
                        continue;
                    }
                    let below = [x, y - 1, z];
                    let sit = self.bname(below).ends_with("_leaves") || self.btag_id(below, crate::tag_bits::TagId::Logs);
                    if sit && self.is_air(p) && self.is_air([x, y + 1, z]) {
                        return Some([f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5]);
                    }
                }
            }
        }
        None
    }

    fn max_build_y(&self) -> i32 {
        self.dim.height.min_y + self.dim.height.sections as i32 * 16 - 1
    }

    /// The climb out of solid blocks.
    fn move_up_out_of_solid(&self, p: [i32; 3]) -> [i32; 3] {
        if !self.is_solid(p) {
            return p;
        }
        let mut q = [p[0], p[1] + 1, p[2]];
        while q[1] <= self.max_build_y() && self.is_solid(q) {
            q[1] += 1;
        }
        q
    }

    /// The climb above solid blocks.
    fn move_up_to_above_solid(&self, p: [i32; 3], amount: i32) -> [i32; 3] {
        if !self.is_solid(p) {
            return p;
        }
        let max = self.max_build_y();
        let mut q = [p[0], p[1] + 1, p[2]];
        while q[1] <= max && self.is_solid(q) {
            q[1] += 1;
        }
        let first = q[1];
        while q[1] <= max && q[1] - first < amount {
            q[1] += 1;
            if self.is_solid(q) {
                q[1] -= 1;
                break;
            }
        }
        q
    }

    // ---------- Bear targeting ----------

    /// Call for help: only adult bears join.
    fn polar_alert(&mut self, m: &Mob) {
        let follow = m.follow_range();
        let p = m.pos;
        let area = rc_physics::Aabb {
            min: [p[0] - follow, p[1] - 10.0, p[2] - follow],
            max: [p[0] + 1.0 + follow, p[1] + 1.0 + 10.0, p[2] + 1.0 + follow],
        };
        let Some((by, _)) = m.last_hurt_by_mob else { return };
        for o in self.mobs.list.iter_mut().flatten() {
            if o.kind == Kind::PolarBear && o.id != m.id && o.target.is_none() && o.touches(&area) && !o.is_baby() {
                o.target = Some(by);
            }
        }
    }

    /// Pursuit of nearby players by a cub: follow range divided by two.
    fn polar_target_continue(&self, base: &mut TBase, m: &mut Mob) -> bool {
        let Some(t) = m.target.or(base.target_mob) else { return false };
        if !self.ent_alive(t) || (t == Ent::Player && !self.player_attackable()) {
            return false;
        }
        let follow = m.follow_range() * 0.5;
        let Some(p) = self.ent_pos(t) else { return false };
        if dist2(m.pos, p) > follow * follow {
            return false;
        }
        if self.line_of_sight(m, t) {
            base.unseen = 0;
        } else {
            base.unseen += 1;
            if base.unseen > reduced(base.unseen_memory) {
                return false;
            }
        }
        m.target = Some(t);
        true
    }

    // ---------- Tools shared with `surface/` ----------

    /// Teleport near the owner (the parrot may land on leaves).
    pub(crate) fn teleport_to_owner_pub(&mut self, m: &mut Mob) {
        self.teleport_to_owner(m);
    }

    /// Panic in progress (tick): hooks specific to the species.
    pub(super) fn surf_panic_tick(&mut self, m: &mut Mob, speed: f64) {
        surface::panic_tick(self, m, speed);
    }
}
