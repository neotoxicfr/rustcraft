//! Own sensors and behaviors of the F13 brains (the hoglin's brain, `Zoglin`), anger, retreat and
//! hunt shared between fellows, current activity.

use super::super::brainy::brain::{Act, Brain, Track, Walk};
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{block_of, dist2};
use super::super::species::Kind;
use super::brain::{Finder, PB, PSn, Who};
use crate::server::Server;

/// Delay between two hunts.
const HUNT_LO: i32 = 600;
const HUNT_HI: i32 = 2400;

fn closer(a: [f64; 3], b: [f64; 3], d: f64) -> bool {
    dist2(a, b) < d * d
}

/// Block center.
fn closer_center(p: [i32; 3], v: [f64; 3], d: f64) -> bool {
    let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
    dist2(c, v) < d * d
}

/// Nemesis of piglins and brutes: wither skeleton or wither (the boss).
pub(crate) fn is_nemesis(k: Kind) -> bool {
    matches!(k, Kind::WitherSkeleton | Kind::Wither)
}

/// Memory of a behavior: stored in the box (`Bk::Pig`).
type PBox = Box<PB>;

impl Server {
    // ---------- Tools ----------

    pub(crate) fn pig_kind(&self, e: Ent) -> Option<Kind> {
        match e {
            Ent::Player => None,
            Ent::Mob(id) => self.mob_ref(id).map(|o| o.kind),
        }
    }

    fn pig_is(&self, e: Ent, k: Kind) -> bool {
        self.pig_kind(e) == Some(k)
    }

    pub(crate) fn pig_uuid(&self, e: Ent) -> Option<u128> {
        match e {
            Ent::Player => Some(self.uuid),
            Ent::Mob(id) => self.mob_ref(id).map(|o| o.uuid),
        }
    }

    pub(crate) fn pig_ent_of(&self, u: u128) -> Option<Ent> {
        if u == self.uuid {
            return Some(Ent::Player);
        }
        self.mobs.get_uuid(u).map(|o| Ent::Mob(o.id))
    }

    /// The "dead or dying" test (entity found again).
    pub(crate) fn pig_dead(&self, e: Ent) -> bool {
        match e {
            Ent::Player => self.life.vit.dead || self.life.vit.health <= 0.0,
            Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| o.health <= 0.0),
        }
    }

    /// The attackable-target test (sight required) with the sensors' shared range.
    pub(crate) fn pig_attackable(&self, m: &mut Mob, b: &Brain, e: Ent) -> bool {
        let invis = b.mem.attack.get() != Some(e);
        let r = self.mobs.pig.sensor_range;
        self.targeting_ok(m, e, true, r, true, invis)
    }

    /// The attackable-target test without line of sight.
    pub(crate) fn pig_attackable_no_los(&self, m: &mut Mob, b: &Brain, e: Ent) -> bool {
        let invis = b.mem.attack.get() != Some(e);
        let r = self.mobs.pig.sensor_range;
        self.targeting_ok(m, e, true, r, false, invis)
    }

    /// The closest-target choice.
    fn pig_nearest_of(&self, m: &Mob, a: Option<Ent>, b: Ent) -> Ent {
        let Some(a) = a else { return b };
        let (pa, pb) = (self.ent_pos(a), self.ent_pos(b));
        match (pa, pb) {
            (Some(pa), Some(pb)) if dist2(m.pos, pa) < dist2(m.pos, pb) => a,
            (Some(_), None) => a,
            _ => b,
        }
    }

    /// The "other target much farther than the current target" test.
    fn pig_much_further(&self, m: &Mob, b: &Brain, other: Ent, more: f64) -> bool {
        let Some(cur) = b.mem.attack.get() else { return false };
        let (Some(pc), Some(po)) = (self.ent_pos(cur), self.ent_pos(other)) else { return false };
        dist2(m.pos, po) > dist2(m.pos, pc) + more * more
    }

    /// Player holding a loved item (main hand or off-hand).
    pub(crate) fn player_holds_loved(&self) -> bool {
        let held = [&self.slots[36 + self.held as usize], &self.slots[45]];
        held.iter().any(|s| s.as_ref().is_some_and(|st| super::items::is_loved(self, st)))
    }

    /// Player's armor in `#piglin_safe_armor`.
    pub(crate) fn player_wears_gold(&self) -> bool {
        (5..9).any(|i| self.slots[i].as_ref().is_some_and(|s| self.item_tag(&s.id, "minecraft:piglin_safe_armor")))
    }

    /// Mob (out of the list during its tick) and its brain.
    pub(crate) fn pig_with(&mut self, id: i32, f: impl FnOnce(&mut Server, &mut Mob, &mut Brain)) {
        let Some(i) = self.mob_index(id) else { return };
        let Some(mut o) = self.mobs.list[i].take() else { return };
        if let Some(mut ob) = o.bm.brain.take() {
            f(self, &mut o, &mut ob);
            o.bm.brain = Some(ob);
        }
        self.mobs.list[i] = Some(o);
    }

    fn pig_visible(&self, m: &mut Mob, b: &mut Brain, e: Ent) -> bool {
        b.mem.visible.has() && self.bz_visible_contains(m, b, e)
    }

    // ---------- Sensors ----------

    pub(crate) fn pig_sense(&mut self, m: &mut Mob, b: &mut Brain, s: PSn) {
        match s {
            PSn::Piglin => self.sense_piglin(m, b),
            PSn::Brute => {
                let f = |s: &Server, e: Ent| s.pig_kind(e).is_some_and(is_nemesis);
                let nem = self.bz_visible_closest(m, b, &f);
                b.mem.pig.nemesis.set_opt(nem);
                let adults = self.pig_nearby_adults(b);
                b.mem.pig.nearby_adult_piglins.set(adults);
            }
            PSn::Hoglin => self.sense_hoglin(m, b),
            PSn::Items => {
                let w = super::items::nearest_wanted(self, m, b);
                b.mem.wanted_item.set_opt(w);
            }
        }
    }

    /// Adult piglins and brutes among nearby entities.
    fn pig_nearby_adults(&self, b: &Brain) -> Vec<Ent> {
        let list = b.mem.living.v.clone().unwrap_or_default();
        list.into_iter()
            .filter(|&e| match e {
                Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| super::is_abstract_piglin(o.kind) && !o.is_baby()),
                Ent::Player => false,
            })
            .collect()
    }

    /// Closest repellent block (8 × 4, Manhattan distance).
    fn pig_repellent(&self, m: &Mob, piglin: bool) -> Option<[i32; 3]> {
        let tag = if piglin { "minecraft:piglin_repellents" } else { "minecraft:hoglin_repellents" };
        super::super::brainy::manhattan_find(m.block_pos(), 8, 4, |p| {
            if !self.btag(p, tag) {
                return false;
            }
            // Soul campfire: only when lit.
            if piglin && self.bname(p) == "minecraft:soul_campfire" {
                return self.v().get_bool(self.st(p), "lit");
            }
            true
        })
    }

    /// The piglin's own sensor.
    fn sense_piglin(&mut self, m: &mut Mob, b: &mut Brain) {
        let rep = self.pig_repellent(m, true);
        b.mem.pig.nearest_repellent.set_opt(rep);
        let mut nemesis = None;
        let mut huntable = None;
        let mut baby_hoglin = None;
        let mut baby_piglin = None;
        let mut zombified = None;
        let mut not_gold = None;
        let mut loved = None;
        let mut hoglins = 0;
        let mut adults = Vec::new();
        let n = b.mem.visible.v.as_ref().map_or(0, |v| v.list.len());
        for i in 0..n {
            let Some(e) = b.mem.visible.v.as_ref().map(|v| v.list[i]) else { break };
            if !self.bz_visible_contains(m, b, e) {
                continue;
            }
            match e {
                Ent::Player => {
                    if not_gold.is_none() && !self.player_wears_gold() && self.bz_mob_can_attack(m, e) {
                        not_gold = Some(e);
                    }
                    if loved.is_none() && self.game_mode != 3 && self.player_holds_loved() {
                        loved = Some(e);
                    }
                }
                Ent::Mob(id) => {
                    let Some((k, baby, hunted)) = self.mob_ref(id).map(|o| (o.kind, o.is_baby(), o.pig.cannot_hunt))
                    else {
                        continue;
                    };
                    match k {
                        Kind::Hoglin => {
                            if baby && baby_hoglin.is_none() {
                                baby_hoglin = Some(e);
                            } else if !baby {
                                hoglins += 1;
                                if huntable.is_none() && !hunted {
                                    huntable = Some(e);
                                }
                            }
                        }
                        Kind::PiglinBrute => adults.push(e),
                        Kind::Piglin => {
                            if baby && baby_piglin.is_none() {
                                baby_piglin = Some(e);
                            } else if !baby {
                                adults.push(e);
                            }
                        }
                        _ => {
                            if nemesis.is_some() || !is_nemesis(k) {
                                if zombified.is_none() && matches!(k, Kind::ZombifiedPiglin | Kind::Zoglin) {
                                    zombified = Some(e);
                                }
                            } else {
                                nemesis = Some(e);
                            }
                        }
                    }
                }
            }
        }
        let _ = baby_piglin;
        let nearby = self.pig_nearby_adults(b);
        let p = &mut b.mem.pig;
        p.nemesis.set_opt(nemesis);
        p.huntable_hoglin.set_opt(huntable);
        p.baby_hoglin.set_opt(baby_hoglin);
        p.zombified.set_opt(zombified);
        p.not_gold_player.set_opt(not_gold);
        p.loved_item_player.set_opt(loved);
        p.nearby_adult_piglins.set(nearby);
        p.piglin_count.set(adults.len() as i32);
        p.visible_adult_piglins.set(adults);
        p.hoglin_count.set(hoglins);
    }

    /// The hoglin's own sensor.
    fn sense_hoglin(&mut self, m: &mut Mob, b: &mut Brain) {
        let rep = self.pig_repellent(m, false);
        b.mem.pig.nearest_repellent.set_opt(rep);
        let mut adult_piglin = None;
        let mut piglins = 0;
        let mut hoglins = Vec::new();
        let n = b.mem.visible.v.as_ref().map_or(0, |v| v.list.len());
        for i in 0..n {
            let Some(e) = b.mem.visible.v.as_ref().map(|v| v.list[i]) else { break };
            let Some((k, baby)) = (match e {
                Ent::Mob(id) => self.mob_ref(id).map(|o| (o.kind, o.is_baby())),
                Ent::Player => None,
            }) else {
                continue;
            };
            if baby || !matches!(k, Kind::Piglin | Kind::Hoglin) {
                continue;
            }
            if !self.bz_visible_contains(m, b, e) {
                continue;
            }
            if k == Kind::Piglin {
                piglins += 1;
                if adult_piglin.is_none() {
                    adult_piglin = Some(e);
                }
            } else {
                hoglins.push(e);
            }
        }
        let p = &mut b.mem.pig;
        p.adult_piglin.set_opt(adult_piglin);
        p.hoglin_count.set(hoglins.len() as i32);
        p.adult_hoglins.set(hoglins);
        p.piglin_count.set(piglins);
    }

    // ---------- Target search ----------

    fn pig_near_zombified(&self, m: &Mob, b: &Brain) -> bool {
        b.mem.pig.zombified.get().and_then(|e| self.ent_pos(e)).is_some_and(|p| closer(m.pos, p, 6.0))
    }

    /// Closest valid attack target (by species).
    fn pig_find_target(&self, m: &mut Mob, b: &mut Brain, f: Finder) -> Option<Ent> {
        match f {
            Finder::Piglin => {
                if self.pig_near_zombified(m, b) {
                    return None;
                }
                if let Some(e) = b.mem.pig.angry_at.get().and_then(|u| self.pig_ent_of(u))
                    && self.pig_attackable_no_los(m, b, e)
                {
                    return Some(e);
                }
                if b.mem.pig.universal_anger.has()
                    && let Some(p) = b.mem.attackable_player.get()
                {
                    return Some(p);
                }
                if let Some(n) = b.mem.pig.nemesis.get() {
                    return Some(n);
                }
                let p = b.mem.pig.not_gold_player.get()?;
                self.pig_attackable(m, b, p).then_some(p)
            }
            Finder::Brute => {
                if let Some(e) = b.mem.pig.angry_at.get().and_then(|u| self.pig_ent_of(u))
                    && self.pig_attackable_no_los(m, b, e)
                {
                    return Some(e);
                }
                b.mem.attackable_player.get().or_else(|| b.mem.pig.nemesis.get())
            }
            Finder::Hoglin => {
                let ok = !b.mem.pig.pacified.has() && !b.mem.breed.has();
                if ok { b.mem.attackable_player.get() } else { None }
            }
            Finder::Zoglin => {
                let n = b.mem.visible.v.as_ref().map_or(0, |v| v.list.len());
                for i in 0..n {
                    let Some(e) = b.mem.visible.v.as_ref().map(|v| v.list[i]) else { break };
                    let k = self.pig_kind(e);
                    if matches!(k, Some(Kind::Zoglin | Kind::Creeper)) {
                        continue;
                    }
                    if self.pig_attackable(m, b, e) && self.bz_visible_contains(m, b, e) {
                        return Some(e);
                    }
                }
                None
            }
        }
    }

    // ---------- Anger and retreat ----------

    fn pig_can_hunt(m: &Mob) -> bool {
        m.kind == Kind::Piglin && !m.pig.cannot_hunt
    }

    /// The hunting pause on hoglins (world random). The brute's brain does not have this
    /// slot: the draw happens, the write is ignored.
    fn pig_dont_hunt(&mut self, kind: Kind, b: &mut Brain) {
        let t = self.mobs.rng.int_between(HUNT_LO, HUNT_HI);
        if kind != Kind::PiglinBrute {
            b.mem.pig.hunted_recently.set_exp(true, i64::from(t));
        }
    }

    /// The choice of a piglin's anger target.
    pub(crate) fn pig_set_anger(&mut self, m: &mut Mob, b: &mut Brain, t: Ent) {
        if !self.pig_attackable_no_los(m, b, t) {
            return;
        }
        b.mem.cant_reach.erase();
        if let Some(u) = self.pig_uuid(t) {
            b.mem.pig.angry_at.set_exp(u, 600);
        }
        if self.pig_is(t, Kind::Hoglin) && Self::pig_can_hunt(m) {
            self.pig_dont_hunt(m.kind, b);
        }
        if t == Ent::Player && self.rule("minecraft:universal_anger", false) {
            b.mem.pig.universal_anger.set_exp(true, 600);
        }
    }

    fn pig_set_anger_nearest_player(&mut self, m: &mut Mob, b: &mut Brain, fallback: Ent) {
        let t = b.mem.attackable_player.get().unwrap_or(fallback);
        self.pig_set_anger(m, b, t);
    }

    /// Nearby adult piglins (hunters only for a hoglin).
    fn pig_broadcast_anger(&mut self, b: &Brain, t: Ent) {
        let list = b.mem.pig.nearby_adult_piglins.v.clone().unwrap_or_default();
        let hoglin_hunted = match t {
            Ent::Mob(id) => {
                self.mob_ref(id).filter(|o| o.kind == Kind::Hoglin).map(|o| !o.is_baby() && !o.pig.cannot_hunt)
            }
            Ent::Player => None,
        };
        for e in list {
            let Ent::Mob(id) = e else { continue };
            self.pig_with(id, |s, o, ob| {
                if let Some(huntable) = hoglin_hunted
                    && (!Self::pig_can_hunt(o) || !huntable)
                {
                    return;
                }
                let cur = ob.mem.pig.angry_at.get().and_then(|u| s.pig_ent_of(u));
                let nearest = s.pig_nearest_of(o, cur, t);
                if cur != Some(nearest) {
                    s.pig_set_anger(o, ob, nearest);
                }
            });
        }
    }

    fn pig_broadcast_universal(&mut self, b: &Brain) {
        let list = b.mem.pig.nearby_adult_piglins.v.clone().unwrap_or_default();
        for e in list {
            let Ent::Mob(id) = e else { continue };
            self.pig_with(id, |s, o, ob| {
                if let Some(p) = ob.mem.attackable_player.get() {
                    s.pig_set_anger(o, ob, p);
                }
            });
        }
    }

    /// The choice of the target to flee with a hunting pause (world random: 5 to 20 s, then hunt).
    fn pig_avoid_and_dont_hunt(&mut self, b: &mut Brain, t: Ent) {
        b.mem.pig.angry_at.erase();
        b.mem.attack.erase();
        b.mem.walk.erase();
        let d = self.mobs.rng.int_between(100, 400);
        b.mem.pig.avoid_target.set_exp(t, i64::from(d));
        self.pig_dont_hunt(Kind::Piglin, b);
    }

    /// The piglin's retreat announcement: adult piglins (not brutes) seen.
    fn piglin_broadcast_retreat(&mut self, b: &Brain, t: Ent) {
        let list = b.mem.pig.visible_adult_piglins.v.clone().unwrap_or_default();
        for e in list {
            let Ent::Mob(id) = e else { continue };
            if !self.pig_is(e, Kind::Piglin) {
                continue;
            }
            self.pig_with(id, |s, o, ob| {
                let n = s.pig_nearest_of(o, ob.mem.pig.avoid_target.get(), t);
                let n = s.pig_nearest_of(o, ob.mem.attack.get(), n);
                s.pig_avoid_and_dont_hunt(ob, n);
            });
        }
    }

    /// The possible retaliation of piglins (piglin and brute).
    fn pig_maybe_retaliate(&mut self, m: &mut Mob, b: &mut Brain, by: Ent) {
        if b.act == Act::Avoid {
            return;
        }
        if !self.pig_attackable_no_los(m, b, by) || self.pig_much_further(m, b, by, 4.0) {
            return;
        }
        if by == Ent::Player && self.rule("minecraft:universal_anger", false) {
            self.pig_set_anger_nearest_player(m, b, by);
            self.pig_broadcast_universal(b);
        } else {
            self.pig_set_anger(m, b, by);
            self.pig_broadcast_anger(b, by);
        }
    }

    /// The piglin's reaction to damage.
    pub(crate) fn piglin_was_hurt_by(&mut self, m: &mut Mob, by: Ent) {
        if self.pig_is(by, Kind::Piglin) {
            return;
        }
        let Some(mut b) = m.bm.brain.take() else { return };
        if m.equipment[1].is_some() {
            self.piglin_stop_holding_offhand(m, false, Some(&b));
        }
        b.mem.pig.celebrate_location.erase();
        b.mem.pig.dancing.erase();
        b.mem.pig.admiring_item.erase();
        if by == Ent::Player {
            b.mem.pig.admiring_disabled.set_exp(true, 400);
        }
        if let Some(a) = b.mem.pig.avoid_target.get() {
            let same = match (a, by) {
                (Ent::Player, Ent::Player) => true,
                _ => self.pig_kind(a).is_some() && self.pig_kind(a) == self.pig_kind(by),
            };
            if !same {
                b.mem.pig.avoid_target.erase();
            }
        }
        if m.is_baby() {
            b.mem.pig.avoid_target.set_exp(by, 100);
            if self.pig_attackable_no_los(m, &b, by) {
                self.pig_broadcast_anger(&b, by);
            }
        } else if self.pig_is(by, Kind::Hoglin) && Self::hoglins_outnumber(&b) {
            self.pig_avoid_and_dont_hunt(&mut b, by);
            self.piglin_broadcast_retreat(&b, by);
        } else {
            self.pig_maybe_retaliate(m, &mut b, by);
        }
        m.bm.brain = Some(b);
    }

    /// Not against a piglin or a brute.
    pub(crate) fn brute_was_hurt_by(&mut self, m: &mut Mob, by: Ent) {
        if self.pig_kind(by).is_some_and(super::is_abstract_piglin) {
            return;
        }
        let Some(mut b) = m.bm.brain.take() else { return };
        self.pig_maybe_retaliate(m, &mut b, by);
        m.bm.brain = Some(b);
    }

    fn hoglins_outnumber(b: &Brain) -> bool {
        let piglins = b.mem.pig.piglin_count.get().unwrap_or(0) + 1;
        let hoglins = b.mem.pig.hoglin_count.get().unwrap_or(0);
        hoglins > piglins
    }

    fn piglins_outnumber(m: &Mob, b: &Brain) -> bool {
        if m.is_baby() {
            return false;
        }
        let piglins = b.mem.pig.piglin_count.get().unwrap_or(0);
        let hoglins = b.mem.pig.hoglin_count.get().unwrap_or(0) + 1;
        piglins > hoglins
    }

    // ---------- Hoglin ----------

    /// The choice of the hoglin's target to flee (world random: 5 to 20 s).
    fn hoglin_set_avoid(&mut self, b: &mut Brain, t: Ent) {
        b.mem.attack.erase();
        b.mem.walk.erase();
        let d = self.mobs.rng.int_between(100, 400);
        b.mem.pig.avoid_target.set_exp(t, i64::from(d));
    }

    fn hoglin_retreat_from(&mut self, m: &Mob, b: &mut Brain, t: Ent) {
        let n = self.pig_nearest_of(m, b.mem.pig.avoid_target.get(), t);
        let n = self.pig_nearest_of(m, b.mem.attack.get(), n);
        self.hoglin_set_avoid(b, n);
    }

    fn hoglin_set_attack(b: &mut Brain, t: Ent) {
        b.mem.cant_reach.erase();
        b.mem.breed.erase();
        b.mem.attack.set_exp(t, 200);
    }

    fn hoglin_broadcast_attack(&mut self, b: &Brain, t: Ent) {
        let list = b.mem.pig.adult_hoglins.v.clone().unwrap_or_default();
        for e in list {
            let Ent::Mob(id) = e else { continue };
            self.pig_with(id, |s, o, ob| {
                if ob.mem.pig.pacified.has() {
                    return;
                }
                let n = s.pig_nearest_of(o, ob.mem.attack.get(), t);
                Self::hoglin_set_attack(ob, n);
            });
        }
    }

    /// The hoglin's reaction after a hit.
    fn hoglin_on_hit_target(&mut self, m: &Mob, b: &mut Brain, t: Ent) {
        if m.is_baby() {
            return;
        }
        if self.pig_is(t, Kind::Piglin) && Self::piglins_outnumber(m, b) {
            self.hoglin_set_avoid(b, t);
            let list = b.mem.pig.adult_hoglins.v.clone().unwrap_or_default();
            for e in list {
                let Ent::Mob(id) = e else { continue };
                self.pig_with(id, |s, o, ob| s.hoglin_retreat_from(o, ob, t));
            }
        } else {
            self.hoglin_broadcast_attack(b, t);
        }
    }

    /// The melee attack of the hoglin and the zoglin: animation (event 4), sound, hoglin reaction,
    /// hit and launch.
    fn hoglin_attack(&mut self, m: &mut Mob, b: &mut Brain, t: Ent) -> bool {
        m.pig.attack_anim = 10;
        self.mob_event(m, 4);
        let snd =
            if m.kind == Kind::Hoglin { "minecraft:entity.hoglin.attack" } else { "minecraft:entity.zoglin.attack" };
        self.make_sound(m, snd);
        if m.kind == Kind::Hoglin {
            self.hoglin_on_hit_target(m, b, t);
        }
        self.hoglin_hurt_and_throw(m, t)
    }

    /// The hoglin's reaction to damage.
    pub(crate) fn hoglin_was_hurt_by(&mut self, m: &mut Mob, by: Ent) {
        let Some(mut b) = m.bm.brain.take() else { return };
        b.mem.pig.pacified.erase();
        b.mem.breed.erase();
        if m.is_baby() {
            self.hoglin_retreat_from(m, &mut b, by);
        } else if (b.act != Act::Avoid || !self.pig_is(by, Kind::Piglin))
            && !self.pig_is(by, Kind::Hoglin)
            && !self.pig_much_further(m, &b, by, 4.0)
            && self.pig_attackable(m, &b, by)
        {
            Self::hoglin_set_attack(&mut b, by);
            self.hoglin_broadcast_attack(&b, by);
        }
        m.bm.brain = Some(b);
    }

    /// Retaliation (unless the current target is much closer).
    pub(crate) fn zoglin_was_hurt_by(&mut self, m: &mut Mob, by: Ent) {
        let Some(mut b) = m.bm.brain.take() else { return };
        if self.bz_mob_can_attack(m, by) && !self.pig_much_further(m, &b, by, 4.0) {
            b.mem.cant_reach.erase();
            b.mem.attack.set_exp(by, 200);
        }
        m.bm.brain = Some(b);
    }

    /// Half the damage + draw (world random), launch.
    pub(crate) fn hoglin_hurt_and_throw(&mut self, m: &mut Mob, t: Ent) -> bool {
        let atk = m.attrs.attack.value() as f32;
        let dmg = if !m.is_baby() && atk as i32 > 0 {
            atk / 2.0 + self.mobs.rng.next_int_bound(atk as i32) as f32
        } else {
            atk
        };
        let ty = "minecraft:mob_attack";
        let hurt = match t {
            Ent::Player => {
                let kb = (m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]);
                self.player_hurt_by_mob(m, ty, dmg, Some(kb))
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return false };
                let Some(mut o) = self.mobs.list[i].take() else { return false };
                let r = self.mob_hurt_by(&mut o, m, ty, dmg);
                self.mobs.list[i] = Some(o);
                r
            }
        };
        if hurt {
            m.last_hurt_mob = Some(t);
            if !m.is_baby() {
                self.hoglin_throw(m, t);
            }
        }
        hurt
    }

    /// Angle from −10 to 10 passed in RADIANS to the yaw rotation (world random).
    pub(crate) fn hoglin_throw(&mut self, m: &Mob, t: Ent) {
        let resist = match t {
            Ent::Player => self.player_attribute("minecraft:knockback_resistance"),
            Ent::Mob(id) => self.mob_ref(id).map_or(0.0, |o| o.attrs.knockback_resistance.value()),
        };
        let eff = m.attrs.attack_knockback.value() - resist;
        if eff <= 0.0 {
            return;
        }
        let Some(tp) = self.ent_pos(t) else { return };
        let (xd, zd) = (tp[0] - m.pos[0], tp[2] - m.pos[2]);
        let angle = (self.mobs.rng.next_int_bound(21) - 10) as f32;
        let hscale = eff * f64::from(self.mobs.rng.next_float() * 0.5 + 0.2);
        let l = (xd * xd + zd * zd).sqrt();
        let (nx, nz) = if l < f64::from(1.0e-5f32) { (0.0, 0.0) } else { (xd / l * hscale, zd / l * hscale) };
        let (c, s) = (f64::from(super::super::mth::cos(angle)), f64::from(super::super::mth::sin(angle)));
        let hx = nx * c + nz * s;
        let hz = nz * c - nx * s;
        let v = eff * f64::from(self.mobs.rng.next_float()) * 0.5;
        match t {
            Ent::Player => {
                let pv = self.mobs.player_vel;
                let nv = [pv[0] + hx, pv[1] + v, pv[2] + hz];
                self.mobs.player_vel = nv;
                let velocity = super::super::track::lp_quantize(nv);
                let _ = self
                    .send(rc_client::Event::Entity(rc_client::EntityEvent::Motion { id: self.player_id, velocity }));
            }
            Ent::Mob(id) => {
                if let Some(i) = self.mob_index(id)
                    && let Some(o) = self.mobs.list[i].as_mut()
                {
                    o.vel = [o.vel[0] + hx, o.vel[1] + v, o.vel[2] + hz];
                    o.track.sync_velocity = true;
                }
            }
        }
    }

    // ---------- Activity ----------

    /// The piglin's activity update.
    pub(crate) fn piglin_update_activity(&mut self, m: &mut Mob, b: &mut Brain) {
        let old = b.act;
        b.first_valid(&[Act::AdmireItem, Act::Fight, Act::Avoid, Act::Celebrate, Act::Ride, Act::Idle]);
        if old != b.act {
            let snd = piglin_activity_sound(self, m, b);
            self.make_sound(m, snd);
        }
        m.aggressive = b.mem.attack.has();
        if !b.mem.pig.ride_target.has() && self.pig_baby_riding_baby(m) {
            self.pig_stop_riding(m);
        }
        if !b.mem.pig.celebrate_location.has() {
            b.mem.pig.dancing.erase();
        }
        m.pig.dancing = b.mem.pig.dancing.has();
    }

    /// The piglin brute's activity update then the possible activity sound (world random, 1.25%).
    pub(crate) fn brute_update_activity(&mut self, m: &mut Mob, b: &mut Brain) {
        let old = b.act;
        b.first_valid(&[Act::Fight, Act::Idle]);
        if old != b.act && b.act == Act::Fight {
            self.make_sound(m, "minecraft:entity.piglin_brute.angry");
        }
        m.aggressive = b.mem.attack.has();
        if f64::from(self.mobs.rng.next_float()) < 0.0125 && b.act == Act::Fight {
            self.make_sound(m, "minecraft:entity.piglin_brute.angry");
        }
    }

    /// The hoglin's activity update.
    pub(crate) fn hoglin_update_activity(&mut self, m: &mut Mob, b: &mut Brain) {
        let old = b.act;
        b.first_valid(&[Act::Fight, Act::Avoid, Act::Idle]);
        if old != b.act {
            let snd = hoglin_activity_sound(self, m, b);
            self.make_sound(m, snd);
        }
        m.aggressive = b.mem.attack.has();
    }

    /// The zoglin's activity update.
    pub(crate) fn zoglin_update_activity(&mut self, m: &mut Mob, b: &mut Brain) {
        let old = b.act;
        b.first_valid(&[Act::Fight, Act::Idle]);
        if b.act == Act::Fight && old != Act::Fight {
            self.make_sound(m, "minecraft:entity.zoglin.angry");
        }
        m.aggressive = b.mem.attack.has();
    }

    // ---------- Riding ----------

    fn pig_baby_riding_baby(&self, m: &Mob) -> bool {
        if !m.is_baby() {
            return false;
        }
        let Some(v) = m.mount.vehicle.and_then(|v| self.mob_ref(v)) else { return false };
        matches!(v.kind, Kind::Piglin | Kind::Hoglin) && v.is_baby()
    }

    /// A mob dismounting from its mount: removed from its mount's passengers.
    pub(crate) fn pig_stop_riding(&mut self, m: &mut Mob) {
        let Some(vid) = m.mount.vehicle.take() else { return };
        if let Some(i) = self.mob_index(vid)
            && let Some(mut v) = self.mobs.list[i].take()
        {
            v.mount.passengers.retain(|p| *p != Ent::Mob(m.id));
            self.send_passengers(&mut v);
            self.mobs.list[i] = Some(v);
        }
    }

    /// A baby on a hoglin mounts the passenger at the top (3 levels).
    fn pig_start_riding(&mut self, m: &mut Mob, target: i32) {
        let mut v = target;
        if m.is_baby() && self.mob_ref(v).is_some_and(|o| o.kind == Kind::Hoglin) {
            let mut counter = 3;
            while counter != 1 {
                let first = self.mob_ref(v).and_then(|o| o.mount.passengers.first().copied());
                match first {
                    Some(Ent::Mob(id)) => v = id,
                    _ => break,
                }
                counter -= 1;
            }
        }
        let Some(i) = self.mob_index(v) else { return };
        let Some(mut veh) = self.mobs.list[i].take() else { return };
        if self.mob_start_riding(m, &mut veh, false) {
            self.send_passengers(&mut veh);
        }
        self.mobs.list[i] = Some(veh);
    }

    // ---------- Behaviors ----------

    /// Instant trigger: true if it acted.
    pub(crate) fn pig_trigger(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PBox, t: i64) -> bool {
        match &mut **p {
            PB::Doors { last_path } => self.pig_doors(m, b, last_path),
            PB::BabyAvoidNemesis => {
                if !m.is_baby() {
                    return false;
                }
                let Some(n) = b.mem.pig.nemesis.get() else { return false };
                let d = self.mobs.rng.int_between(100, 140);
                b.mem.pig.avoid_target.set_exp(n, i64::from(d));
                true
            }
            PB::AvoidZombified => {
                if !self.pig_near_zombified(m, b) {
                    return false;
                }
                let Some(z) = b.mem.pig.zombified.get() else { return false };
                let d = self.mobs.rng.int_between(100, 140);
                b.mem.pig.avoid_target.set_exp(z, i64::from(d));
                true
            }
            PB::StopHoldingItem => {
                // Any item that parries (`blocks_attacks` component, stack or default) stays in hand.
                let parries = m.equipment[1].as_ref().is_some_and(|s| self.stack_has(s, "minecraft:blocks_attacks"));
                if m.equipment[1].is_some() && !parries {
                    self.piglin_stop_holding_offhand(m, true, Some(&*b));
                    return true;
                }
                false
            }
            PB::StartAdmiring => {
                let Some(id) = b.mem.wanted_item.get() else { return false };
                let loved =
                    self.life.drops.iter().find(|d| d.id == id).is_some_and(|d| super::items::is_loved(self, &d.stack));
                if !loved {
                    return false;
                }
                b.mem.pig.admiring_item.set_exp(true, 119);
                true
            }
            PB::StartCelebrating => {
                let Some(target) = b.mem.attack.get() else { return false };
                if !self.pig_dead(target) {
                    return false;
                }
                // Random seeded from the game time (killed hoglin only).
                if self.pig_is(target, Kind::Hoglin) && JRand::new(self.time).next_float() < 0.1 {
                    b.mem.pig.dancing.set_exp(true, 300);
                }
                if let Some(tp) = self.ent_pos(target) {
                    b.mem.pig.celebrate_location.set_exp(block_of(tp), 300);
                }
                if target != Ent::Player || self.rule("minecraft:forgive_dead_players", true) {
                    b.mem.attack.erase();
                    b.mem.pig.angry_at.erase();
                }
                true
            }
            PB::StopAngryIfDead => {
                if let Some(e) = b.mem.pig.angry_at.get().and_then(|u| self.pig_ent_of(u))
                    && self.pig_dead(e)
                    && (e != Ent::Player || self.rule("minecraft:forgive_dead_players", true))
                {
                    b.mem.pig.angry_at.erase();
                }
                true
            }
            PB::LookAt { who, range } => {
                let max = f64::from(*range * *range);
                let me = m.pos;
                let w = *who;
                let passengers = m.mount.passengers.clone();
                let loved = self.player_holds_loved();
                let f = move |s: &Server, e: Ent| {
                    let kind_ok = match w {
                        Who::Any => true,
                        Who::Player => e == Ent::Player,
                        Who::Piglin => s.pig_is(e, Kind::Piglin),
                        Who::Brute => s.pig_is(e, Kind::PiglinBrute),
                        Who::LovedItemPlayer => e == Ent::Player && loved,
                    };
                    kind_ok && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= max) && !passengers.contains(&e)
                };
                let Some(target) = self.bz_visible_closest(m, b, &f) else { return false };
                b.mem.look.set(Track::ent(target, true));
                true
            }
            PB::StartAttacking(f) => {
                let f = *f;
                if f == Finder::Piglin && m.is_baby() {
                    return false;
                }
                let Some(target) = self.pig_find_target(m, b, f) else { return false };
                if !self.bz_mob_can_attack(m, target) {
                    return false;
                }
                b.mem.attack.set(target);
                b.mem.cant_reach.erase();
                true
            }
            PB::StartHunting => {
                if m.kind != Kind::Piglin || m.pig.cannot_hunt || m.is_baby() {
                    return false;
                }
                let friends = b.mem.pig.visible_adult_piglins.v.clone().unwrap_or_default();
                let hunted = friends.iter().any(|&e| match e {
                    Ent::Mob(id) => self
                        .mob_ref(id)
                        .and_then(|o| o.bm.brain.as_ref())
                        .is_some_and(|ob| ob.mem.pig.hunted_recently.has()),
                    Ent::Player => false,
                });
                if hunted {
                    return false;
                }
                let Some(h) = b.mem.pig.huntable_hoglin.get() else { return false };
                self.pig_set_anger(m, b, h);
                self.pig_dont_hunt(m.kind, b);
                self.pig_broadcast_anger(b, h);
                for e in friends {
                    if let Ent::Mob(id) = e {
                        self.pig_with(id, |s, o, ob| s.pig_dont_hunt(o.kind, ob));
                    }
                }
                true
            }
            PB::AwayFromRepellent { speed, dist, interrupt } => {
                let Some(r) = b.mem.pig.nearest_repellent.get() else { return false };
                let from = [f64::from(r[0]) + 0.5, f64::from(r[1]), f64::from(r[2]) + 0.5];
                self.pig_walk_away(m, b, from, *speed, *dist, *interrupt)
            }
            PB::AwayFromEntity { from, speed, dist, interrupt, adult_only } => {
                if *adult_only && m.is_baby() {
                    return false;
                }
                let e = match from {
                    super::brain::PK::AvoidTarget => b.mem.pig.avoid_target.get(),
                    _ => b.mem.pig.adult_piglin.get(),
                };
                let Some(pos) = e.and_then(|e| self.ent_pos(e)) else { return false };
                self.pig_walk_away(m, b, pos, *speed, *dist, *interrupt)
            }
            PB::BabyRide { ticker } => {
                if !m.is_baby() {
                    return false;
                }
                let fire = if *ticker == 0 {
                    *ticker = self.mobs.rng.int_between(200, 800) - 1;
                    false
                } else {
                    *ticker -= 1;
                    *ticker == 0
                };
                if !fire {
                    return false;
                }
                let Some(h) = b.mem.pig.baby_hoglin.get() else { return false };
                let d = self.mobs.rng.int_between(200, 600);
                b.mem.pig.ride_target.set_exp(h, i64::from(d));
                true
            }
            PB::Stroll { speed, h, v } => {
                let p = self.land_random_pos(m, *h, *v);
                b.mem.walk.set_opt(p.map(|p| Walk::to(p, *speed, 0)));
                true
            }
            PB::InteractWith { who, speed, stop } => {
                let w = *who;
                let valid = move |s: &Server, e: Ent| match w {
                    Who::Piglin => s.pig_is(e, Kind::Piglin),
                    Who::Brute => s.pig_is(e, Kind::PiglinBrute),
                    _ => false,
                };
                // Memory containing a valid target: a fellow seen, then the closest within 8 blocks.
                if self.bz_visible_closest(m, b, &valid).is_none() {
                    return false;
                }
                let me = m.pos;
                let close = move |s: &Server, e: Ent| valid(s, e) && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= 64.0);
                if let Some(e) = self.bz_visible_closest(m, b, &close) {
                    b.mem.pig.interaction_target.set(e);
                    b.mem.look.set(Track::ent(e, true));
                    b.mem.walk.set(Walk { target: Track::ent(e, false), speed: *speed, close: *stop });
                }
                true
            }
            PB::WalkFromLook { speed, close, if_no_loved } => {
                if *if_no_loved && b.mem.pig.loved_item_player.has() {
                    return false;
                }
                let Some(l) = b.mem.look.get() else { return false };
                b.mem.walk.set(Walk { target: l, speed: *speed, close: *close });
                true
            }
            PB::LookAndInteract { range } => {
                let max = f64::from(*range * *range);
                let me = m.pos;
                let f = move |s: &Server, e: Ent| e == Ent::Player && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= max);
                let Some(e) = self.bz_visible_closest(m, b, &f) else { return false };
                b.mem.pig.interaction_target.set(e);
                b.mem.look.set(Track::ent(e, true));
                true
            }
            PB::StopAttackingIfInvalid { finder } => {
                let Some(target) = b.mem.attack.get() else { return false };
                let tired = b.mem.cant_reach.get().is_some_and(|c| self.time - c > 200);
                let still = self.bz_mob_can_attack(m, target) && !tired && self.ent_alive(target) && {
                    match finder {
                        Some(f) => self.pig_find_target(m, b, *f) == Some(target),
                        None => true,
                    }
                };
                if !still {
                    b.mem.attack.erase();
                }
                true
            }
            PB::BackUpIfTooClose { dist, speed } => {
                if !super::items::holds(m, "minecraft:crossbow") {
                    return false;
                }
                let Some(target) = b.mem.attack.get() else { return false };
                let near = self.ent_pos(target).is_some_and(|p| closer(m.pos, p, f64::from(*dist)));
                if !near || !self.pig_visible(m, b, target) {
                    return false;
                }
                b.mem.look.set(Track::ent(target, true));
                m.move_ctl.op = MoveOp::Strafe;
                m.move_ctl.strafe_fwd = -*speed;
                m.move_ctl.strafe_right = 0.0;
                m.move_ctl.speed_mod = 0.25;
                m.yaw = m.head_yaw;
                true
            }
            PB::WalkToAttack { speed } => {
                let Some(target) = b.mem.attack.get() else { return false };
                if self.pig_visible(m, b, target) && self.pig_within_attack_range(m, target, 1) {
                    b.mem.walk.erase();
                } else {
                    b.mem.look.set(Track::ent(target, true));
                    b.mem.walk.set(Walk { target: Track::ent(target, false), speed: *speed, close: 0 });
                }
                true
            }
            PB::Melee { cd, adult } => {
                if let Some(a) = adult
                    && *a == m.is_baby()
                {
                    return false;
                }
                let Some(target) = b.mem.attack.get() else { return false };
                // The crossbow or a spear (either hand).
                let ranged = m.kind == Kind::Piglin
                    && (super::items::holds(m, "minecraft:crossbow") || super::spear::holds_kinetic(m));
                if ranged || !self.within_melee(m, target) || !self.pig_visible(m, b, target) {
                    return false;
                }
                b.mem.look.set(Track::ent(target, true));
                self.mob_swing(m);
                if matches!(m.kind, Kind::Hoglin | Kind::Zoglin) {
                    self.hoglin_attack(m, b, target);
                } else {
                    self.mob_attack(m, target);
                }
                b.mem.attack_cooling.set_exp((), i64::from(*cd));
                true
            }
            PB::RememberHoglinKill => {
                if let Some(target) = b.mem.attack.get()
                    && self.pig_is(target, Kind::Hoglin)
                    && self.pig_dead(target)
                {
                    self.pig_dont_hunt(m.kind, b);
                }
                true
            }
            PB::EraseAttackIfNearZombified => {
                if self.pig_near_zombified(m, b) {
                    b.mem.attack.erase();
                    return true;
                }
                false
            }
            PB::GoToCelebrate { close, speed, dancing } => {
                if *dancing != m.pig.dancing {
                    return false;
                }
                let Some(loc) = b.mem.pig.celebrate_location.get() else { return false };
                let bp = m.block_pos();
                let d = [loc[0] - bp[0], loc[1] - bp[1], loc[2] - bp[2]];
                let d2 = i64::from(d[0]).pow(2) + i64::from(d[1]).pow(2) + i64::from(d[2]).pow(2);
                if d2 >= i64::from(*close * *close) {
                    let ox = self.mobs.rng.next_int_bound(3) - 1;
                    let oz = self.mobs.rng.next_int_bound(3) - 1;
                    let p = [loc[0] + ox, loc[1], loc[2] + oz];
                    b.mem.look.set(Track::block(p));
                    b.mem.walk.set(Walk { target: Track::block(p), speed: *speed, close: *close });
                }
                true
            }
            PB::GoToWantedItem => {
                if super::items::loved_in_offhand(self, m) || b.mem.pickup_cd.has() || !m.can_pickup {
                    return false;
                }
                let Some(id) = b.mem.wanted_item.get() else { return false };
                let Some(ip) = self.life.drops.iter().find(|d| d.id == id).map(|d| d.pos) else { return false };
                // In range and within the world border.
                if !closer(ip, m.pos, 9.0) || !self.may_interact(super::super::mth::block_of(ip)) {
                    return false;
                }
                b.mem.walk.set(Walk { target: Track::Item { id, eye: false }, speed: 1.0, close: 0 });
                b.mem.look.set(Track::Item { id, eye: true });
                true
            }
            PB::StopAdmiringTooFar => {
                if m.equipment[1].is_some() {
                    return false;
                }
                let near = b
                    .mem
                    .wanted_item
                    .get()
                    .and_then(|id| self.life.drops.iter().find(|d| d.id == id))
                    .is_some_and(|d| closer(d.pos, m.pos, 9.0));
                if near {
                    return false;
                }
                b.mem.pig.admiring_item.erase();
                true
            }
            PB::StopAdmiringTired => {
                if m.equipment[1].is_some() {
                    return false;
                }
                match b.mem.pig.time_reaching_item.get() {
                    None => b.mem.pig.time_reaching_item.set(0),
                    Some(n) if n > 200 => {
                        b.mem.pig.admiring_item.erase();
                        b.mem.pig.time_reaching_item.erase();
                        b.mem.pig.disable_walk_to_item.set_exp(true, 200);
                    }
                    Some(n) => b.mem.pig.time_reaching_item.set(n + 1),
                }
                true
            }
            PB::EraseAvoidIfStop => {
                let stop = if m.kind == Kind::Hoglin {
                    !m.is_baby() && !Self::piglins_outnumber(m, b)
                } else {
                    self.piglin_wants_to_stop_fleeing(b)
                };
                if stop {
                    b.mem.pig.avoid_target.erase();
                    return true;
                }
                false
            }
            PB::Mount { speed } => {
                if m.mount.vehicle.is_some() {
                    return false;
                }
                let Some(r) = b.mem.pig.ride_target.get() else { return false };
                let Some(rp) = self.ent_pos(r) else { return false };
                if closer(rp, m.pos, 1.0) {
                    if let Ent::Mob(id) = r {
                        self.pig_start_riding(m, id);
                    }
                } else {
                    b.mem.look.set(Track::ent(r, true));
                    b.mem.walk.set(Walk { target: Track::ent(r, false), speed: *speed, close: 1 });
                }
                true
            }
            PB::RideLook { rng, order } => {
                if m.mount.vehicle.is_none() {
                    return false;
                }
                // Key −U^(1/weight), all of weight 1.
                let mut keyed: Vec<(f64, usize)> = (0..4).map(|i| (-f64::from(rng.next_float()), i)).collect();
                keyed.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
                *order = keyed.into_iter().map(|k| k.1).collect();
                for &i in order.iter() {
                    let who = match i {
                        0 => Who::Player,
                        1 => Who::Piglin,
                        2 => Who::Any,
                        _ => break,
                    };
                    if b.mem.look.has() || !b.mem.visible.has() {
                        continue;
                    }
                    let mut k = Box::new(PB::LookAt { who, range: 8.0 });
                    if self.pig_trigger(m, b, &mut k, t) {
                        break;
                    }
                }
                true
            }
            PB::Dismount => {
                let cur = m.mount.vehicle.map(Ent::Mob);
                let target = b.mem.pig.ride_target.get();
                let Some(v) = cur.or(target) else { return false };
                let valid = self.ent_alive(v) && self.ent_pos(v).is_some_and(|p| closer(p, m.pos, 8.0));
                if valid && !self.piglin_wants_to_stop_riding(b, v) {
                    return false;
                }
                self.pig_stop_riding(m);
                b.mem.pig.ride_target.erase();
                true
            }
            PB::BecomePassive => {
                b.mem.pig.pacified.set_exp(true, 200);
                b.mem.attack.erase();
                true
            }
            PB::EraseAttackIfBreeding => {
                if b.mem.breed.has() {
                    b.mem.attack.erase();
                    return true;
                }
                false
            }
            PB::StrollToHome { next } => {
                let Some(h) = b.mem.pig.home.get() else { return false };
                let here = super::super::villager::brain::DimKey::of(&self.dim.name);
                if h.dim != here || !closer_center(h.pos, m.pos, 100.0) {
                    return false;
                }
                if t <= *next {
                    return true;
                }
                b.mem.walk.set(Walk { target: Track::block(h.pos), speed: 0.6, close: 2 });
                *next = t + 80;
                true
            }
            PB::StrollAroundHome { next } => {
                let Some(h) = b.mem.pig.home.get() else { return false };
                let here = super::super::villager::brain::DimKey::of(&self.dim.name);
                if h.dim != here || !closer_center(h.pos, m.pos, 5.0) {
                    return false;
                }
                if t <= *next {
                    return true;
                }
                let p = self.land_random_pos(m, 8, 6);
                b.mem.walk.set_opt(p.map(|p| Walk::to(p, 0.6, 1)));
                *next = t + 180;
                true
            }
            PB::Crossbow { .. } | PB::SpearApproach | PB::SpearAttack { .. } | PB::SpearRetreat { .. } => false,
        }
    }

    /// 10 "opposite" position tries (16 × 7).
    fn pig_walk_away(
        &mut self,
        m: &mut Mob,
        b: &mut Brain,
        from: [f64; 3],
        speed: f32,
        dist: i32,
        interrupt: bool,
    ) -> bool {
        let cur = b.mem.walk.get();
        if cur.is_some() && !interrupt {
            return false;
        }
        if !closer(m.pos, from, f64::from(dist)) {
            return false;
        }
        if let Some(w) = cur
            && w.speed == speed
            && let Some(tp) = self.bz_track_pos(w.target)
        {
            let cd = [tp[0] - m.pos[0], tp[1] - m.pos[1], tp[2] - m.pos[2]];
            let ad = [from[0] - m.pos[0], from[1] - m.pos[1], from[2] - m.pos[2]];
            if cd[0] * ad[0] + cd[1] * ad[1] + cd[2] * ad[2] < 0.0 {
                return false;
            }
        }
        for _ in 0..10 {
            if let Some(p) = self.pig_land_pos_away(m, 0.0, 16.0, 7, from) {
                b.mem.walk.set(Walk::to(p, speed, 0));
                break;
            }
        }
        true
    }

    /// Walk value of the mob as weight.
    pub(crate) fn pig_land_pos_away(
        &self,
        m: &mut Mob,
        min_h: f64,
        max_h: f64,
        v: i32,
        from: [f64; 3],
    ) -> Option<[f64; 3]> {
        let mut dir = [m.pos[0] - from[0], m.pos[2] - from[2]];
        let dy = m.pos[1] - from[1];
        if dir[0] * dir[0] + dy * dy + dir[1] * dir[1] == 0.0 {
            dir = [m.rng.next_double() - 0.5, m.rng.next_double() - 0.5];
        }
        let mut best: Option<(f32, [i32; 3])> = None;
        for _ in 0..10 {
            // The direction draw in a cone (π/2) then the position draw toward a direction.
            let center = super::super::mth::atan2(dir[1], dir[0]) - f64::from(std::f32::consts::FRAC_PI_2);
            let yr = center + f64::from(2.0 * m.rng.next_float() - 1.0) * f64::from(std::f32::consts::FRAC_PI_2);
            let t = m.rng.next_double().sqrt();
            let d = super::super::mth::rpos_dist(t, min_h, max_h);
            let (xt, zt) = (-d * yr.sin(), d * yr.cos());
            if xt.abs() > max_h || zt.abs() > max_h {
                continue;
            }
            let yt = m.rng.next_int_bound(2 * v + 1) - v;
            let p = [
                (xt.floor() + m.pos[0]).floor() as i32,
                (f64::from(yt) + m.pos[1]).floor() as i32,
                (zt.floor() + m.pos[2]).floor() as i32,
            ];
            let hgt = self.dim.height;
            if p[1] < hgt.min_y || p[1] >= hgt.min_y + hgt.sections as i32 * 16 || !self.nav_stable(m, p) {
                continue;
            }
            // Climbing out of the solid, then water and malus refused.
            let mut q = p;
            let max = hgt.min_y + hgt.sections as i32 * 16 - 1;
            if self.is_solid(q) {
                q[1] += 1;
                while q[1] <= max && self.is_solid(q) {
                    q[1] += 1;
                }
            }
            if self.mob_fluid(q) == super::super::env::Fluid::Water || self.pos_has_malus(m, q) {
                continue;
            }
            let w = self.walk_target_value(m, q);
            if w > best.map_or(f32::NEG_INFINITY, |b| b.0) {
                best = Some((w, q));
            }
        }
        best.map(|b| [f64::from(b.1[0]) + 0.5, f64::from(b.1[1]), f64::from(b.1[2]) + 0.5])
    }

    /// Piglin crossbow (range 8 − margin), otherwise melee.
    fn pig_within_attack_range(&self, m: &Mob, t: Ent, margin: i32) -> bool {
        let main = m.equipment[0].as_ref().map(|s| s.id.as_str());
        if m.kind == Kind::Piglin && main == Some("minecraft:crossbow") {
            let r = f64::from(8 - margin);
            return self.ent_pos(t).is_some_and(|p| closer(m.pos, p, r));
        }
        self.within_melee(m, t)
    }

    /// The piglin's wish to stop fleeing.
    fn piglin_wants_to_stop_fleeing(&self, b: &Brain) -> bool {
        let Some(a) = b.mem.pig.avoid_target.get() else { return true };
        match self.pig_kind(a) {
            Some(Kind::Hoglin) => !Self::hoglins_outnumber(b),
            Some(Kind::ZombifiedPiglin | Kind::Zoglin) => b.mem.pig.zombified.get() != Some(a),
            _ => false,
        }
    }

    /// The wish to dismount.
    fn piglin_wants_to_stop_riding(&self, b: &Brain, v: Ent) -> bool {
        let Ent::Mob(id) = v else { return false };
        let Some(o) = self.mob_ref(id) else { return true };
        let hurt_v = o.bm.brain.as_ref().is_some_and(|ob| ob.mem.hurt_by.has());
        !o.is_baby()
            || !o.alive()
            || b.mem.hurt_by.has()
            || hurt_v
            || (o.kind == Kind::Piglin && o.mount.vehicle.is_none())
    }

    /// Doors of the path opened, those passed closed again.
    fn pig_doors(&mut self, m: &mut Mob, b: &mut Brain, last: &mut Option<Vec<[i32; 3]>>) -> bool {
        let Some(path) = m.nav.path.clone() else { return false };
        if path.next == 0 || path.is_done() {
            return false;
        }
        // Last node seen and countdown stored in `last`: [node], [delay, 0, 0].
        let state = last.get_or_insert_with(|| vec![[i32::MIN; 3], [0; 3]]);
        let next = path.nodes[path.next].0;
        if state[0] == next {
            state[1][0] = 20;
        } else {
            state[1][0] -= 1;
            if state[1][0] > 0 {
                return false;
            }
        }
        state[0] = next;
        let from = path.nodes[path.next - 1].0;
        if self.is_mob_door(from) {
            self.set_door_open(from, true, m.id);
            let d = b.mem.pig.doors_to_close.v.get_or_insert_with(Vec::new);
            if !d.contains(&from) {
                d.push(from);
            }
        }
        if self.is_mob_door(next) && !self.v().get_bool(self.st(next), "open") {
            self.set_door_open(next, true, m.id);
            let d = b.mem.pig.doors_to_close.v.get_or_insert_with(Vec::new);
            if !d.contains(&next) {
                d.push(next);
            }
        }
        if let Some(doors) = b.mem.pig.doors_to_close.v.take() {
            let mut keep = Vec::new();
            for d in doors {
                if d == from || d == next {
                    keep.push(d);
                    continue;
                }
                if !closer_center(d, m.pos, 3.0) || !self.is_mob_door(d) || !self.v().get_bool(self.st(d), "open") {
                    continue;
                }
                let others = self.mobs.list.iter().flatten().any(|o| {
                    o.kind == m.kind
                        && o.id != m.id
                        && closer_center(d, o.pos, 2.0)
                        && o.nav.path.as_ref().is_some_and(|p| {
                            !p.is_done() && p.next > 0 && (p.nodes[p.next - 1].0 == d || p.nodes[p.next].0 == d)
                        })
                });
                if !others {
                    self.set_door_open(d, false, m.id);
                }
            }
            b.mem.pig.doors_to_close.v = Some(keep);
        }
        true
    }

    // ---------- Crossbow (behavior with a duration) ----------

    fn pig_crossbow_ok(&self, m: &mut Mob, b: &mut Brain) -> bool {
        let Some(t) = b.mem.attack.get() else { return false };
        super::items::holds(m, "minecraft:crossbow")
            && self.pig_visible(m, b, t)
            && self.pig_within_attack_range(m, t, 0)
    }

    pub(crate) fn pig_check(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PBox) -> bool {
        match &**p {
            PB::Crossbow { .. } => self.pig_crossbow_ok(m, b),
            PB::SpearApproach | PB::SpearAttack { .. } | PB::SpearRetreat { .. } => self.spear_check(m, b, p),
            _ => true,
        }
    }

    pub(crate) fn pig_start(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PBox) {
        if matches!(&**p, PB::SpearApproach | PB::SpearAttack { .. } | PB::SpearRetreat { .. }) {
            self.spear_start(m, b, p);
        }
    }

    pub(crate) fn pig_continue(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PBox) -> bool {
        match &**p {
            PB::Crossbow { .. } => b.mem.attack.has() && self.pig_crossbow_ok(m, b),
            PB::SpearApproach | PB::SpearAttack { .. } | PB::SpearRetreat { .. } => self.spear_continue(m, b, p),
            _ => false,
        }
    }

    pub(crate) fn pig_tick(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PBox) {
        if matches!(&**p, PB::SpearApproach | PB::SpearAttack { .. } | PB::SpearRetreat { .. }) {
            self.spear_tick(m, b, p);
            return;
        }
        let PB::Crossbow { state, delay } = &mut **p else { return };
        let Some(target) = b.mem.attack.get() else { return };
        b.mem.look.set(Track::ent(target, true));
        match *state {
            0 => {
                m.use_ticks = 0;
                *state = 1;
                m.pig.charging = true;
            }
            1 => {
                if m.use_ticks < 0 {
                    *state = 0;
                }
                if m.use_ticks.max(0) >= self.crossbow_charge_duration(m) {
                    // Last tick of use (loading), then stop.
                    self.crossbow_use_tick(m);
                    m.use_ticks = -1;
                    *state = 2;
                    *delay = 20 + m.rng.next_int_bound(20);
                    m.pig.charging = false;
                }
            }
            2 => {
                *delay -= 1;
                if *delay == 0 {
                    *state = 3;
                }
            }
            _ => {
                self.crossbow_shoot(m, target);
                m.no_action_time = 0;
                *state = 0;
            }
        }
    }

    /// The use stops; the "in use" item is then empty, so
    /// the loaded projectiles stay in the crossbow (like the game).
    pub(crate) fn pig_stop(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PBox) {
        if matches!(&**p, PB::SpearApproach | PB::SpearAttack { .. } | PB::SpearRetreat { .. }) {
            self.spear_stop(m, b, p);
            return;
        }
        if let PB::Crossbow { .. } = &**p {
            if m.use_ticks >= 0 {
                m.use_ticks = -1;
            }
            if super::items::holds(m, "minecraft:crossbow") {
                m.pig.charging = false;
            }
        }
    }
}

/// The choice of the piglin's sound according to activity.
pub(crate) fn piglin_activity_sound(s: &Server, m: &Mob, b: &Brain) -> &'static str {
    let near_avoid = b.mem.pig.avoid_target.get().and_then(|e| s.ent_pos(e)).is_some_and(|p| closer(p, m.pos, 12.0));
    match b.act {
        Act::Fight => "minecraft:entity.piglin.angry",
        _ if super::is_converting(s, m) => "minecraft:entity.piglin.retreat",
        Act::Avoid if near_avoid => "minecraft:entity.piglin.retreat",
        Act::AdmireItem => "minecraft:entity.piglin.admiring_item",
        Act::Celebrate => "minecraft:entity.piglin.celebrate",
        _ if b.mem.pig.loved_item_player.has() => "minecraft:entity.piglin.jealous",
        _ if b.mem.pig.nearest_repellent.has() => "minecraft:entity.piglin.retreat",
        _ => "minecraft:entity.piglin.ambient",
    }
}

/// The choice of the hoglin's sound according to activity.
pub(crate) fn hoglin_activity_sound(s: &Server, m: &Mob, b: &Brain) -> &'static str {
    if b.act == Act::Avoid || super::is_converting(s, m) {
        "minecraft:entity.hoglin.retreat"
    } else if b.act == Act::Fight {
        "minecraft:entity.hoglin.angry"
    } else if b.mem.pig.nearest_repellent.has() {
        "minecraft:entity.hoglin.retreat"
    } else {
        "minecraft:entity.hoglin.ambient"
    }
}
