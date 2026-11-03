//! G5-F04: enchantment effects read from the pack and applied like the game in a damage
//! context: protection of the worn equipment (`damage_protection`), armor effectiveness and
//! damage per fallen block of the weapon (`armor_effectiveness`: Breach,
//! `smash_damage_per_fallen_block`: Density), and the after-hit effects (`post_attack`: Thorns,
//! Fire Aspect, Bane of Arthropods, Wind Burst…), those of the victim's equipment first, then
//! those of the attacker's weapon.
//!
//! Conditions: the loot condition evaluator (`progress::pred`), with `random_chance` drawn here
//! on the world's random (chance by enchantment level). Value effects: the victim's random.

use serde_json::Value;

use crate::commands::ents::E;
use crate::mobs::mob::{Ent, Mob, SLOT_NAMES};
use crate::player::Stack;
use crate::progress::pred::{Ctx, Dmg, Who};
use crate::projectiles::ench::level_value;
use crate::randsrc::Rand;
use crate::server::Server;

/// One side of a hit: the current player, a mob held outside the list by the caller, or another
/// struck entity (armor stand, minecart, crystal, another local-network player) seen only by its
/// identifier and position: no equipment read, no effect applied to it.
pub(crate) enum Side<'a> {
    Player,
    Mob(&'a mut Mob),
    Other(E, [f64; 3]),
    /// As the attacker only: the victim itself (hit by its own projectile). `Hit::side` and
    /// `Hit::split` resolve it to the victim.
    Itself,
}

impl Side<'_> {
    fn ent(&self) -> Option<Ent> {
        match self {
            Side::Player => Some(Ent::Player),
            Side::Mob(m) => Some(Ent::Mob(m.id)),
            Side::Other(..) | Side::Itself => None,
        }
    }

    fn who(&self) -> Who<'_> {
        match self {
            Side::Player => Who::E(E::Player),
            Side::Mob(m) => Who::Mob(m),
            Side::Other(e, _) => Who::E(*e),
            Side::Itself => unreachable!("resolved to the victim by Hit::side"),
        }
    }
}

/// Role of an entity in a hit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Victim,
    Attacker,
}

/// A landed hit, for its after-hit effects.
pub(crate) struct Hit<'a> {
    pub ty: &'a str,
    pub victim: Side<'a>,
    /// Entity responsible for the hit (`attacker`); absent or gone: none.
    pub attacker: Option<Side<'a>>,
    /// Hit dealt by the attacker itself (melee); false: through a projectile or another entity.
    pub direct: bool,
}

impl<'a> Hit<'a> {
    fn side(&self, r: Role) -> Option<&Side<'a>> {
        match (r, &self.attacker) {
            (Role::Victim, _) | (Role::Attacker, Some(Side::Itself)) => Some(&self.victim),
            (Role::Attacker, a) => a.as_ref(),
        }
    }

    /// The side `r` mutable and the other side readable (none when both are the same entity).
    fn split(&mut self, r: Role) -> (Option<&mut Side<'a>>, Option<&Side<'a>>) {
        match (r, &mut self.attacker) {
            (_, Some(Side::Itself)) => (Some(&mut self.victim), None),
            (Role::Victim, a) => (Some(&mut self.victim), a.as_ref()),
            (Role::Attacker, a) => (a.as_mut(), Some(&self.victim)),
        }
    }
}

/// Equipment slot of the player for a slot index (`SLOT_NAMES`): none for body and saddle.
fn player_slot(held: i32, k: usize) -> Option<usize> {
    Some(match k {
        0 => 36 + held as usize,
        1 => 45,
        2 => 8,
        3 => 7,
        4 => 6,
        5 => 5,
        _ => return None,
    })
}

/// Number provider of a `random_chance` condition (number, `enchantment_level`, constant).
fn chance_of(v: &Value, lvl: i32) -> f32 {
    if let Some(n) = v.as_f64() {
        return n as f32;
    }
    match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
        "enchantment_level" => level_value(&v["amount"], lvl),
        "constant" => v["value"].as_f64().unwrap_or(0.0) as f32,
        _ => 0.0,
    }
}

/// Float provider (constant or uniform: sound volume, contact damage), sampled on `rng`.
pub(crate) fn sample(v: &Value, rng: &mut dyn Rand) -> f32 {
    if let Some(n) = v.as_f64() {
        return n as f32;
    }
    match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
        "uniform" => {
            let (lo, hi) = (v["min_inclusive"].as_f64().unwrap_or(0.0) as f32, v["max_exclusive"].as_f64().unwrap_or(0.0) as f32);
            rng.next_float() * (hi - lo) + lo
        }
        _ => v["value"].as_f64().unwrap_or(1.0) as f32,
    }
}

impl Server {
    /// Requirements of an effect: the loot conditions, `random_chance` drawn on `rng` with a chance
    /// by enchantment level `lvl`.
    pub(crate) fn ench_req(&self, c: &Value, ctx: &Ctx<'_>, lvl: i32, rng: &mut dyn Rand) -> bool {
        if let Value::Array(a) = c {
            return a.iter().all(|x| self.ench_req(x, ctx, lvl, rng));
        }
        let ty = c.get("type").or_else(|| c.get("condition")).and_then(Value::as_str).unwrap_or("");
        match ty.trim_start_matches("minecraft:") {
            "all_of" => c["terms"].as_array().is_none_or(|t| t.iter().all(|x| self.ench_req(x, ctx, lvl, rng))),
            "any_of" => c["terms"].as_array().is_some_and(|t| t.iter().any(|x| self.ench_req(x, ctx, lvl, rng))),
            "inverted" => !self.ench_req(&c["term"], ctx, lvl, rng),
            "random_chance" => rng.next_float() < chance_of(&c["chance"], lvl),
            _ => self.p_condition(c, ctx),
        }
    }

    /// Damage context of a hit on `victim` (attacker, direct or not).
    pub(crate) fn damage_ctx<'w>(&self, ty: &str, victim: Who<'w>, origin: [f64; 3], attacker: Option<Who<'w>>, direct: bool) -> Ctx<'w> {
        let direct_attacker = if direct { attacker } else { None };
        Ctx {
            this: Some(victim),
            origin: Some(origin),
            attacker,
            direct_attacker,
            damage: Some(Dmg { ty: ty.to_owned(), source: attacker, direct: direct_attacker, pos: None }),
            ..Default::default()
        }
    }

    /// Damage context of a hit seen from the victim, the attacker designated by `by` (mob of the
    /// list or held copy); direct when the type has no direct entity of its own.
    pub(crate) fn hurt_ctx<'w>(&'w self, ty: &str, victim: Who<'w>, origin: [f64; 3], by: Option<Ent>) -> Ctx<'w> {
        let attacker = match by {
            Some(Ent::Player) => Some(Who::E(E::Player)),
            Some(Ent::Mob(id)) => self.mob_ref(id).map(Who::Mob),
            None => None,
        };
        let direct = attacker.is_some() && crate::loot::direct_of(ty) == Some(None);
        self.damage_ctx(ty, victim, origin, attacker, direct)
    }

    /// Weapon of the damage source: the main hand of an attacker that hits directly.
    pub(crate) fn source_weapon(&self, ty: &str, by: Option<Ent>) -> Option<Stack> {
        if crate::loot::direct_of(ty) != Some(None) {
            return None;
        }
        match by? {
            Ent::Player => self.slots[36 + self.held as usize].clone(),
            Ent::Mob(id) => self.mob_ref(id)?.equipment[0].clone(),
        }
    }

    /// Protection of the worn equipment against a hit: sum of the `damage_protection` effects of the
    /// enchantments whose slots cover the slot worn, conditions held (`world`: their random draws;
    /// `own`: the victim's random for the value effects).
    pub(crate) fn ench_protection(
        &self,
        gear: &[(usize, Option<&Stack>)],
        ctx: &Ctx<'_>,
        world: &mut dyn Rand,
        own: &mut dyn Rand,
    ) -> f32 {
        let mut p = 0.0;
        for &(k, s) in gear {
            let Some(s) = s.filter(|s| enchanted(&self.life.items, s)) else { continue };
            for (ench, lvl) in crate::enchanting::read_comp(&self.life.items, s, "minecraft:enchantments") {
                let Some(def) = self.life.pack.get("enchantment", &ench) else { continue };
                if !slot_matches(&def, SLOT_NAMES[k]) {
                    continue;
                }
                let Some(Value::Array(list)) = def["effects"].get("minecraft:damage_protection") else { continue };
                for x in list {
                    if x.get("requirements").is_none_or(|r| self.ench_req(r, ctx, lvl, world)) {
                        p = self.value_effect(&x["effect"], lvl, p, own);
                    }
                }
            }
        }
        p
    }

    /// Value of a conditional component of a weapon in a damage context (every enchantment of the
    /// item, whatever its slots, like the game for the held weapon).
    pub(crate) fn ench_weapon_value(
        &self,
        w: &Stack,
        component: &str,
        base: f32,
        ctx: &Ctx<'_>,
        world: &mut dyn Rand,
        own: &mut dyn Rand,
    ) -> f32 {
        let mut v = base;
        if !enchanted(&self.life.items, w) {
            return v;
        }
        for (ench, lvl) in crate::enchanting::read_comp(&self.life.items, w, "minecraft:enchantments") {
            let Some(def) = self.life.pack.get("enchantment", &ench) else { continue };
            let Some(Value::Array(list)) = def["effects"].get(component) else { continue };
            for x in list {
                if x.get("requirements").is_none_or(|r| self.ench_req(r, ctx, lvl, world)) {
                    v = self.value_effect(&x["effect"], lvl, v, own);
                }
            }
        }
        v
    }

    /// Armor fraction of a hit after the weapon's `armor_effectiveness` (Breach), clamped to
    /// `[0, 1]`; unchanged without a weapon.
    pub(crate) fn armor_fraction(&self, frac: f32, weapon: Option<&Stack>, ctx: &Ctx<'_>, own: &mut dyn Rand) -> f32 {
        let Some(w) = weapon else { return frac };
        let mut world = self.mobs.rng;
        self.ench_weapon_value(w, "minecraft:armor_effectiveness", frac, ctx, &mut world, own).clamp(0.0, 1.0)
    }

    /// After-hit effects of a landed hit: the victim's equipment (all slots, enchanted = victim),
    /// then the attacker's weapon in the main hand (enchanted = attacker).
    pub(crate) fn post_attack(&mut self, hit: &mut Hit<'_>, weapon: Option<&Stack>) {
        for k in 0..8 {
            let s = match &hit.victim {
                Side::Player => player_slot(self.held, k).and_then(|i| self.slots[i].as_ref()),
                Side::Mob(m) => m.equipment[k].as_ref(),
                Side::Other(..) | Side::Itself => None,
            };
            // Only an enchanted item is copied (the effects may change the slot while they run).
            let Some(s) = s.filter(|s| enchanted(&self.life.items, s)).cloned() else { continue };
            self.post_attack_item(hit, &s, Role::Victim, k);
        }
        if let Some(w) = weapon
            && hit.attacker.is_some()
        {
            self.post_attack_item(hit, w, Role::Attacker, 0);
        }
    }

    /// After-hit effects of the player's weapon on a struck entity that is neither the player nor
    /// a mob (`victim` at `pos`: armor stand, minecart, crystal, another local-network player);
    /// the victim's equipment is not read.
    pub(crate) fn player_weapon_post_attack(&mut self, victim: E, pos: [f64; 3], ty: &str, weapon: Option<&Stack>) {
        let mut hit = Hit { ty, victim: Side::Other(victim, pos), attacker: Some(Side::Player), direct: true };
        self.post_attack(&mut hit, weapon);
    }

    /// After-hit effects of a melee hit of the mob `m` (held outside the list) on `t`: the
    /// victim's equipment, then the weapon `weapon`.
    pub(crate) fn melee_post_attack(&mut self, m: &mut Mob, t: Ent, ty: &str, weapon: Option<&Stack>) {
        match t {
            Ent::Player => {
                let mut hit = Hit { ty, victim: Side::Player, attacker: Some(Side::Mob(m)), direct: true };
                self.post_attack(&mut hit, weapon);
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return };
                let Some(mut o) = self.mobs.list[i].take() else { return };
                let mut hit = Hit { ty, victim: Side::Mob(o.as_mut()), attacker: Some(Side::Mob(m)), direct: true };
                self.post_attack(&mut hit, weapon);
                self.mobs.list[i] = Some(o);
            }
        }
    }

    /// After-hit effects of a hit on `t` caused by `by` through a projectile or another entity: the
    /// victim's equipment only (thorns on the shooter); the shooter is looked up in the list.
    pub(crate) fn post_attack_by(&mut self, t: Ent, by: Option<Ent>, ty: &str) {
        // Hit by its own projectile (`by == t`): the victim is its own attacker, like the game (its
        // thorns hurt it and wear the piece).
        let take = |s: &mut Server, id: i32| s.mob_index(id).and_then(|i| Some((i, s.mobs.list[i].take()?)));
        let mut victim = match t {
            Ent::Player => None,
            Ent::Mob(id) => match take(self, id) {
                Some(v) => Some(v),
                None => return,
            },
        };
        let mut shooter = match by {
            Some(Ent::Mob(id)) if Some(Ent::Mob(id)) != Some(t) => take(self, id),
            _ => None,
        };
        let attacker = match (by, shooter.as_mut()) {
            (Some(b), _) if b == t => Some(Side::Itself),
            (Some(Ent::Player), _) => Some(Side::Player),
            (_, Some((_, a))) => Some(Side::Mob(a.as_mut())),
            _ => None,
        };
        let victim_side = match victim.as_mut() {
            Some((_, v)) => Side::Mob(v.as_mut()),
            None => Side::Player,
        };
        let mut hit = Hit { ty, victim: victim_side, attacker, direct: false };
        self.post_attack(&mut hit, None);
        if let Some((i, a)) = shooter {
            self.mobs.list[i] = Some(a);
        }
        if let Some((i, v)) = victim {
            self.mobs.list[i] = Some(v);
        }
    }

    /// The target `t` of a smash, seen by its knockback.
    pub(crate) fn struck(&self, t: Ent) -> Option<crate::mace::Struck> {
        match t {
            Ent::Player => Some(crate::mace::Struck {
                ent: Ent::Player,
                uuid: self.uuid,
                pos: self.pos,
                bbox: self.player_aabb(),
                on_ground: self.on_ground,
                on_pos: self.on_pos(self.pos, self.player_support(), 1.0e-5),
            }),
            Ent::Mob(id) => self.mob_ref(id).map(|m| crate::mace::Struck::mob(self, m)),
        }
    }

    fn post_attack_item(&mut self, hit: &mut Hit<'_>, s: &Stack, owner: Role, k: usize) {
        let enchanted = if owner == Role::Victim { "victim" } else { "attacker" };
        for (ench, lvl) in crate::enchanting::read_comp(&self.life.items, s, "minecraft:enchantments") {
            let Some(def) = self.life.pack.get("enchantment", &ench) else { continue };
            if !slot_matches(&def, SLOT_NAMES[k]) {
                continue;
            }
            let Some(Value::Array(list)) = def["effects"].get("minecraft:post_attack") else { continue };
            for x in list {
                if x["enchanted"].as_str() != Some(enchanted) {
                    continue;
                }
                if let Some(r) = x.get("requirements") {
                    let mut world = self.mobs.rng;
                    let ok = {
                        let ctx = self.hit_ctx(hit);
                        self.ench_req(r, &ctx, lvl, &mut world)
                    };
                    self.mobs.rng = world;
                    if !ok {
                        continue;
                    }
                }
                let target = match x["affected"].as_str() {
                    Some("victim") => Role::Victim,
                    Some("attacker") => Role::Attacker,
                    // The damaging entity: the attacker itself on a direct hit (a projectile is
                    // not held here).
                    Some("damaging_entity") if hit.direct => Role::Attacker,
                    _ => continue,
                };
                if hit.side(target).is_none() {
                    continue;
                }
                self.entity_effect(hit, &x["effect"], lvl, owner, k, target);
            }
        }
    }

    fn hit_ctx<'h>(&'h self, hit: &'h Hit<'_>) -> Ctx<'h> {
        let origin = self.side_pos(&hit.victim);
        self.damage_ctx(hit.ty, hit.victim.who(), origin, hit.side(Role::Attacker).map(Side::who), hit.direct)
    }

    fn side_pos(&self, s: &Side<'_>) -> [f64; 3] {
        match s {
            Side::Player => self.pos,
            Side::Mob(m) => m.pos,
            Side::Other(_, p) => *p,
            Side::Itself => unreachable!("resolved to the victim by Hit::side"),
        }
    }

    /// Entity effect applied to the side `target`; the enchanted item belongs to `owner`, slot `k`.
    fn entity_effect(&mut self, hit: &mut Hit<'_>, e: &Value, lvl: i32, owner: Role, k: usize, target: Role) {
        match e["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "all_of" => {
                for x in e["effects"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    self.entity_effect(hit, x, lvl, owner, k, target);
                }
            }
            "damage_entity" => self.fx_damage(hit, e, lvl, owner, target),
            "change_item_damage" => {
                let n = level_value(&e["amount"], lvl) as i32;
                match hit.split(owner).0 {
                    Some(Side::Player) => {
                        if let Some(i) = player_slot(self.held, k)
                            && self.slots[i].as_ref().is_some_and(|s| self.life.items.damageable(s))
                        {
                            self.hurt_item(i, n);
                        }
                    }
                    Some(Side::Mob(m)) => self.mob_hurt_slot(m, k, n),
                    _ => {}
                }
            }
            "ignite" => {
                let ticks = (level_value(&e["duration"], lvl) * 20.0).floor() as i32;
                match hit.split(target).0 {
                    Some(Side::Player) => self.player_ignite(ticks),
                    Some(Side::Mob(m)) => crate::inside_blocks::mob_ignite(m, ticks),
                    _ => {}
                }
            }
            "apply_mob_effect" => self.fx_mob_effect(hit, e, lvl, target),
            "explode" => self.fx_explode(hit, e, lvl, target),
            "play_sound" => self.fx_sound(hit, e, lvl, target),
            // Not used by after-hit effects in the game data (summoning is the trident's,
            // handled at its hit): ignored.
            _ => {}
        }
    }

    /// `damage_entity`: damage between the bounds (target's random) caused by the item's owner.
    fn fx_damage(&mut self, hit: &mut Hit<'_>, e: &Value, lvl: i32, owner: Role, target: Role) {
        let (lo, hi) = (level_value(&e["min_damage"], lvl), level_value(&e["max_damage"], lvl));
        let ty = crate::commands::args::id(e["damage_type"].as_str().unwrap_or("minecraft:generic"));
        let src_ent = hit.side(owner).and_then(Side::ent);
        let src_pos = hit.side(owner).map(|s| self.side_pos(s));
        let same = owner == target;
        let (t, other) = hit.split(target);
        let src = if same { None } else { other };
        match t {
            Some(Side::Player) => {
                let dmg = self.life.rng.next_float() * (hi - lo) + lo;
                match src {
                    Some(Side::Mob(o)) => {
                        let kb = (o.pos[0] - self.pos[0], o.pos[2] - self.pos[2]);
                        self.player_hurt_by_mob(o, &ty, dmg, Some(kb));
                    }
                    _ => {
                        let kb = src_pos.map(|p| (p[0] - self.pos[0], p[2] - self.pos[2]));
                        self.player_hurt_kb(&ty, dmg, kb);
                    }
                }
            }
            Some(Side::Mob(m)) => {
                let dmg = m.rng.next_float() * (hi - lo) + lo;
                match src {
                    Some(Side::Mob(o)) => {
                        let o: &Mob = o;
                        self.ghost_push(o);
                        self.mob_hurt(m, &ty, dmg, Some(Ent::Mob(o.id)), Some(o.pos));
                        self.ghost_pop();
                    }
                    _ => {
                        self.mob_hurt(m, &ty, dmg, src_ent, src_pos);
                    }
                }
            }
            _ => {}
        }
    }

    /// `apply_mob_effect`: one effect of the set, duration and amplifier drawn on the target's random.
    fn fx_mob_effect(&mut self, hit: &mut Hit<'_>, e: &Value, lvl: i32, target: Role) {
        let ids: Vec<String> = match &e["to_apply"] {
            Value::String(s) => vec![crate::commands::args::id(s)],
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(crate::commands::args::id)).collect(),
            _ => Vec::new(),
        };
        if ids.is_empty() {
            return;
        }
        let lv = |k: &str| level_value(&e[k], lvl);
        let (d0, d1, a0, a1) = (lv("min_duration"), lv("max_duration"), lv("min_amplifier"), lv("max_amplifier"));
        let draw = |r: &mut dyn Rand| {
            let id = ids[r.next_int_bound(ids.len() as i32) as usize].clone();
            let ticks = rc_data::java_math::round_f32_to_i32((r.next_float() * (d1 - d0) + d0) * 20.0);
            let amp = rc_data::java_math::round_f32_to_i32(r.next_float() * (a1 - a0) + a0).max(0);
            (id, ticks, amp)
        };
        match hit.split(target).0 {
            Some(Side::Player) => {
                let mut r = self.life.rng;
                let (id, ticks, amp) = draw(&mut r);
                self.life.rng = r;
                self.give_player_effect(&id, ticks, amp);
            }
            Some(Side::Mob(m)) => {
                let (id, ticks, amp) = draw(&mut m.rng);
                self.give_mob_effect(m, &id, ticks, amp);
            }
            _ => {}
        }
    }

    /// `explode` without damage (Wind Burst): wind burst at the target, pushing the held mobs too.
    fn fx_explode(&mut self, hit: &mut Hit<'_>, e: &Value, lvl: i32, target: Role) {
        if e.get("damage_type").is_some() || e["block_interaction"].as_str() != Some("trigger") {
            return;
        }
        let Some(t) = hit.side(target) else { return };
        let off = |i: usize| e["offset"].get(i).and_then(Value::as_f64).unwrap_or(0.0);
        let p = self.side_pos(t);
        let center = [p[0] + off(0), p[1] + off(1), p[2] + off(2)];
        let radius = level_value(&e["radius"], lvl).max(0.0);
        let kb = e.get("knockback_multiplier").map_or(1.0, |v| f64::from(level_value(v, lvl)));
        let sound = match &e["sound"] {
            Value::String(s) => crate::commands::args::id(s),
            o => crate::commands::args::id(o["sound_id"].as_str().unwrap_or("")),
        };
        let mut held: Vec<&mut Mob> = Vec::new();
        if let Side::Mob(m) = &mut hit.victim {
            held.push(m);
        }
        if let Some(Side::Mob(m)) = &mut hit.attacker {
            held.push(m);
        }
        // Without an author: neither the wind charge's fall grace nor a projectile owner.
        let cause = self.prog.expl_cause_next.take();
        self.wind_burst_with(None, None, center, radius, kb, &sound, &mut held);
        self.prog.expl_cause_next = cause;
    }

    /// `play_sound` at the target (not silent): sound of the level, volume and pitch drawn on its
    /// random, its sound category.
    fn fx_sound(&mut self, hit: &mut Hit<'_>, e: &Value, lvl: i32, target: Role) {
        let list: Vec<String> = match &e["sound"] {
            Value::String(s) => vec![crate::commands::args::id(s)],
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().or_else(|| x["sound_id"].as_str()).map(crate::commands::args::id))
                .collect(),
            o => o["sound_id"].as_str().map(crate::commands::args::id).into_iter().collect(),
        };
        if list.is_empty() {
            return;
        }
        let name = list[(lvl - 1).clamp(0, list.len() as i32 - 1) as usize].clone();
        let (pos, cat, vol, pitch) = match hit.split(target).0 {
            Some(Side::Player) => {
                let mut r = self.life.rng;
                let (v, p) = (sample(&e["volume"], &mut r), sample(&e["pitch"], &mut r));
                self.life.rng = r;
                (self.pos, 7, v, p)
            }
            Some(Side::Mob(m)) => {
                if m.x.silent {
                    return;
                }
                let (v, p) = (sample(&e["volume"], &mut m.rng), sample(&e["pitch"], &mut m.rng));
                (m.pos, Self::mob_sound_source(m), v, p)
            }
            _ => return,
        };
        self.world_sound(&name, cat, pos, vol, pitch);
    }
}

/// The stack may carry enchantments: its own component, otherwise a non-empty default one (read
/// in place: neither copy of the default value nor formatted key).
pub(crate) fn enchanted(items: &crate::items::Items, s: &Stack) -> bool {
    if crate::items::component(s, "minecraft:enchantments").is_some() {
        return true;
    }
    if crate::items::component(s, "!minecraft:enchantments").is_some() {
        return false;
    }
    items.defaults(&s.id)["minecraft:enchantments"].as_object().is_some_and(|o| !o.is_empty())
}

/// The enchantment `def` applies to the slot `slot` (`slots` groups).
fn slot_matches(def: &Value, slot: &str) -> bool {
    def["slots"]
        .as_array()
        .is_some_and(|a| a.iter().any(|g| g.as_str().is_some_and(|g| crate::attrs::group_has(g.trim_start_matches("minecraft:"), slot))))
}
