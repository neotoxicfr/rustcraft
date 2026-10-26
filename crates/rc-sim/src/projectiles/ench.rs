//! Enchantment effects read from the pack (`enchantment/*.json`, effect components) for
//! throwing weapons and the rod: power and impaling (`damage`), punch (`knockback`),
//! infinity (`ammo_use`), multishot (`projectile_count`, `projectile_spread`), piercing,
//! loyalty, riptide, quick charge, flame (`projectile_spawned`), lure and luck of the sea.
//! Values per level and conditions evaluated like the game (useful subset here).

use serde_json::Value;

use crate::items::Items;
use crate::player::Stack;
use crate::server::Server;

/// Condition context: type of the target (`this`), of the direct attacker, tool tested.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx<'a> {
    pub this: Option<&'a str>,
    pub direct: Option<&'a str>,
    pub tool: Option<&'a Stack>,
    /// Attacker (author of the damage): entity type.
    pub attacker: Option<&'a str>,
}

/// Slot (`mainhand`, `feet`…) included in an enchantment slot group.
pub fn slot_in_group(group: &str, slot: &str) -> bool {
    match group.trim_start_matches("minecraft:") {
        "any" => true,
        "hand" => matches!(slot, "mainhand" | "offhand"),
        "armor" => matches!(slot, "feet" | "legs" | "chest" | "head" | "body"),
        g => g == slot,
    }
}

/// Level of an enchantment on a stack (`enchantments` component, item default included).
pub fn level(items: &Items, s: &Stack, id: &str) -> i32 {
    crate::enchanting::read_comp(items, s, "minecraft:enchantments")
        .into_iter()
        .find(|(k, _)| k == id)
        .map_or(0, |(_, l)| l)
}

/// Value per level.
pub fn level_value(v: &Value, lvl: i32) -> f32 {
    if let Some(n) = v.as_f64() {
        return n as f32;
    }
    let f = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
    match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
        "linear" => f("base") + f("per_level_above_first") * (lvl - 1) as f32,
        "constant" => f("value"),
        "clamped" => level_value(&v["value"], lvl).clamp(f("min"), f("max")),
        "fraction" => {
            let d = level_value(&v["denominator"], lvl);
            if d == 0.0 { 0.0 } else { level_value(&v["numerator"], lvl) / d }
        }
        "levels_squared" => (lvl * lvl) as f32 + f("added"),
        "lookup" => {
            let vals = v["values"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            match vals.get((lvl - 1).max(0) as usize) {
                Some(x) if lvl >= 1 && (lvl as usize) <= vals.len() => x.as_f64().unwrap_or(0.0) as f32,
                _ => level_value(&v["fallback"], lvl),
            }
        }
        _ => 0.0,
    }
}

impl Server {
    /// Entity type in an entity tag.
    pub(crate) fn entity_type_in(&self, name: &str, tag: &str) -> bool {
        let Some(idx) =
            self.data.registry("minecraft:entity_type").and_then(|r| r.entries.iter().position(|n| n == name))
        else {
            return false;
        };
        let tag = tag.trim_start_matches('#');
        let short = tag.trim_start_matches("minecraft:");
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:entity_type")
            .and_then(|(_, t)| {
                t.iter().find(|(n, _)| n == tag || n == short || n.trim_start_matches("minecraft:") == short)
            })
            .is_some_and(|(_, ids)| ids.contains(&(idx as i32)))
    }

    fn entity_matches(&self, name: Option<&str>, pred: &Value) -> bool {
        let Some(name) = name else { return false };
        let ty = pred.get("minecraft:entity_type").or_else(|| pred.get("type"));
        if let Some(t) = ty {
            let ok = match t {
                Value::String(s) => match s.strip_prefix('#') {
                    Some(tag) => self.entity_type_in(name, tag),
                    None => s == name,
                },
                Value::Array(a) => a.iter().any(|x| x.as_str() == Some(name)),
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        // Location (visible sky…): not evaluated here, accepted.
        true
    }

    fn ench_item_matches(&self, s: Option<&Stack>, pred: &Value) -> bool {
        let Some(s) = s else { return false };
        match &pred["items"] {
            Value::String(x) => match x.strip_prefix('#') {
                Some(tag) => self.life.items.has_tag(tag, &s.id),
                None => *x == s.id,
            },
            Value::Array(a) => a.iter().any(|x| x.as_str() == Some(s.id.as_str())),
            Value::Null => true,
            _ => false,
        }
    }

    /// Effect condition (`requirements`).
    fn ench_condition(&self, c: &Value, ctx: &Ctx<'_>) -> bool {
        match c["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "all_of" => c["terms"].as_array().is_none_or(|t| t.iter().all(|x| self.ench_condition(x, ctx))),
            "any_of" => c["terms"].as_array().is_some_and(|t| t.iter().any(|x| self.ench_condition(x, ctx))),
            "inverted" => !self.ench_condition(&c["term"], ctx),
            "entity_properties" => {
                let who = match c["entity"].as_str().unwrap_or("this") {
                    "this" => ctx.this,
                    "direct_attacker" => ctx.direct,
                    "attacker" => ctx.attacker,
                    _ => None,
                };
                self.entity_matches(who, &c["predicate"])
            }
            "match_tool" => self.ench_item_matches(ctx.tool, &c["predicate"]),
            "weather_check" => {
                let w = &self.life.weather;
                c["raining"].as_bool().is_none_or(|r| r == w.raining)
                    && c["thundering"].as_bool().is_none_or(|t| t == w.thundering)
            }
            _ => false,
        }
    }

    /// Value effect applied to `v`.
    fn value_effect(&self, e: &Value, lvl: i32, v: f32, rng: &mut dyn crate::randsrc::Rand) -> f32 {
        match e["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "add" => v + level_value(&e["value"], lvl),
            "multiply" => v * level_value(&e["factor"], lvl),
            "set" => level_value(&e["value"], lvl),
            "remove_binomial" => {
                let chance = level_value(&e["chance"], lvl);
                let mut out = v;
                for _ in 0..(v as i32) {
                    if rng.next_float() < chance {
                        out -= 1.0;
                    }
                }
                out
            }
            "all_of" => {
                let mut out = v;
                for x in e["effects"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    out = self.value_effect(x, lvl, out, rng);
                }
                out
            }
            _ => v,
        }
    }

    /// Enchantments of the stack, in component order, with their definition.
    fn ench_defs(&self, s: &Stack) -> Vec<(i32, std::sync::Arc<Value>)> {
        crate::enchanting::read_comp(&self.life.items, s, "minecraft:enchantments")
            .into_iter()
            .filter_map(|(id, l)| Some((l, self.life.pack.get("enchantment", &id)?)))
            .collect()
    }

    /// Value modified by a conditional effect component (list of effects with conditions).
    pub(crate) fn ench_value(&self, s: &Stack, component: &str, base: f32, ctx: &Ctx<'_>) -> f32 {
        let mut v = base;
        let mut rng = self.life.rng;
        for (lvl, def) in self.ench_defs(s) {
            let Some(list) = def["effects"].get(component) else { continue };
            match list {
                Value::Array(a) => {
                    for x in a {
                        if x.get("requirements").is_none_or(|r| self.ench_condition(r, ctx)) {
                            v = self.value_effect(&x["effect"], lvl, v, &mut rng);
                        }
                    }
                }
                // Non-conditional component (riptide, quick charge).
                o @ Value::Object(_) => v = self.value_effect(o, lvl, v, &mut rng),
                _ => {}
            }
        }
        v
    }

    /// Effect carried by the highest level: element of the list.
    pub(crate) fn ench_highest(&self, s: &Stack, component: &str) -> Option<Value> {
        let mut best: Option<(i32, Value)> = None;
        for (lvl, def) in self.ench_defs(s) {
            let Some(Value::Array(a)) = def["effects"].get(component) else { continue };
            if best.as_ref().is_none_or(|b| lvl > b.0)
                && let Some(x) = a.get((lvl.clamp(1, a.len() as i32) - 1) as usize)
            {
                best = Some((lvl, x.clone()));
            }
        }
        best.map(|b| b.1)
    }

    /// Ignition duration granted to the projectile by the weapon (`projectile_spawned` → `ignite`).
    pub(crate) fn ench_spawn_fire(&self, s: &Stack) -> i32 {
        let mut ticks = 0;
        for (lvl, def) in self.ench_defs(s) {
            let Some(Value::Array(a)) = def["effects"].get("minecraft:projectile_spawned") else { continue };
            for x in a {
                if x["effect"]["type"].as_str() == Some("minecraft:ignite") {
                    let secs = level_value(&x["effect"]["duration"], lvl);
                    ticks = ticks.max((secs * 20.0).floor() as i32);
                }
            }
        }
        ticks
    }
}

impl Server {
    /// The modification of damage by enchantments (target, direct attacker = the projectile).
    pub(crate) fn ench_damage(
        &self,
        w: &Stack,
        direct: super::PKind,
        target: Option<crate::mobs::species::Kind>,
        base: f32,
    ) -> f32 {
        let this = match target {
            Some(k) => Some(k.info().name),
            None => Some("minecraft:player"),
        };
        let ctx = Ctx { this, direct: Some(direct.name()), tool: Some(w), ..Ctx::default() };
        self.ench_value(w, "minecraft:damage", base, &ctx)
    }

    /// The modification of knockback by enchantments (punch), base 0.
    pub(crate) fn ench_knockback(
        &self,
        w: &Stack,
        direct: super::PKind,
        target: Option<crate::mobs::species::Kind>,
    ) -> f32 {
        let this = Some(target.map_or("minecraft:player", |k| k.info().name));
        let ctx = Ctx { this, direct: Some(direct.name()), tool: Some(w), ..Ctx::default() };
        self.ench_value(w, "minecraft:knockback", 0.0, &ctx)
    }

    /// Stack carrying an enchantment with an effect component (curse of vanishing:
    /// `prevent_equipment_drop`).
    pub(crate) fn ench_has(&self, s: &Stack, component: &str) -> bool {
        self.ench_defs(s).iter().any(|(_, d)| d["effects"].get(component).is_some())
    }

    /// Chance to drop equipment on death, modified by `equipment_drops` effects:
    /// first those of the victim's equipment (`enchanted` victim), then those of the
    /// attacker's equipment (`enchanted` attacker, e.g. Looting: +0.01 per level), slot by
    /// slot, each enchantment only in the slots it targets.
    pub(crate) fn ench_equipment_drop_chance(
        &self,
        victim: &[(&str, Option<&Stack>)],
        attacker: &[(&str, Option<&Stack>)],
        ctx: &Ctx<'_>,
        chance: f32,
        rng: &mut dyn crate::randsrc::Rand,
    ) -> f32 {
        let mut v = chance;
        for (who, gear) in [("victim", victim), ("attacker", attacker)] {
            for (slot, s) in gear {
                let Some(s) = s else { continue };
                for (lvl, def) in self.ench_defs(s) {
                    let slots = def["slots"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                    if !slots.iter().any(|g| g.as_str().is_some_and(|g| slot_in_group(g, slot))) {
                        continue;
                    }
                    let Some(Value::Array(list)) = def["effects"].get("minecraft:equipment_drops") else { continue };
                    for x in list {
                        if x["enchanted"].as_str() == Some(who)
                            && x.get("requirements").is_none_or(|r| self.ench_condition(r, ctx))
                        {
                            v = self.value_effect(&x["effect"], lvl, v, rng);
                        }
                    }
                }
            }
        }
        v
    }
}
