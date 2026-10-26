//! Loot tables, conditions and number providers (`server-gameplay.md` § 5), read from the
//! jar's base pack. Order of random draws kept (conditions evaluated when entries
//! are expanded, then functions in order), to follow the game on the same
//! generator.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::datapack::DataPack;
use crate::items::Items;
use crate::player::Stack;
use crate::randsrc::Rand;

/// Candidate of a draw: entry, weight, item drawn from an expanded tag entry.
type Cand<'v> = (&'v Value, i32, Option<String>);

/// Furnace smelting of a stack (`furnace_smelt`).
type SmeltFn<'a> = dyn Fn(&Stack) -> Option<Stack> + 'a;

/// Draw parameters.
#[derive(Default)]
pub struct Ctx<'a> {
    /// Block concerned: id and properties.
    pub block: Option<(&'a str, Vec<(String, String)>)>,
    /// Tool (present, possibly empty, when a player breaks).
    pub tool: Option<Option<&'a Stack>>,
    pub explosion: Option<f32>,
    pub block_entity: Option<&'a Nbt>,
    pub luck: f32,
    // S4: mob loot.
    pub killed_by_player: bool,
    pub on_fire: bool,
    pub baby: bool,
    /// Looting level of the killer's weapon.
    pub looting: i32,
    pub damage_type: Option<String>,
    /// Sheep: color (name) and sheared.
    pub sheep: Option<(String, bool)>,
    /// Known entity components (`minecraft:mooshroom/variant` → `red`…), F02.
    pub comps: Vec<(String, String)>,
    /// F03: size of a slime (predicate `type_specific/cube_mob`).
    pub cube_size: Option<i32>,
    /// F07: raid raider (in a raid, captain) — predicate `type_specific/raider`.
    pub raider: Option<(bool, bool)>,
    /// F10: source entity of the damage (type, known components) — `damage_source_properties`.
    pub source_entity: Option<(String, Vec<(String, String)>)>,
    /// S8: fishing bobber in open water (predicate `type_specific/fishing_hook`).
    pub open_water: Option<bool>,
    /// S8: biome of the origin (`location_check` on `biomes`).
    pub biome: Option<String>,
    /// Type of the `this` entity (predicate `entity_type`, e.g. charged creeper heads).
    pub this_type: Option<String>,
    /// Author of the damage (`attacker`): entity type and type tags that contain it.
    pub attacker: Option<(String, Vec<String>)>,
    /// W8c: full world evaluator for the draw-free conditions this context does not
    /// know (offset location, direct attacker and its equipment, source tags…).
    pub world: Option<&'a dyn Fn(&Value) -> bool>,
    /// W8c: type of the direct entity of the damage when it is not alive (projectile).
    pub direct_type: Option<String>,
    /// W8c: furnace smelting result of a stack (`furnace_smelt`).
    pub smelt: Option<&'a SmeltFn<'a>>,
}

pub struct Loot<'a> {
    pub pack: &'a DataPack,
    pub items: &'a Items,
    /// Block tags (sorted ids) and block names (for `match_block`).
    pub block_tag: &'a dyn Fn(&str, &str) -> bool,
}

/// Shapes the loot context evaluates itself (the rest goes to the world when it is lent):
/// source = type and components of the source entity; attacker = its type; killed entity =
/// flags, components, sheep, slime, raider, bobber, type; location = biomes only.
fn locally_known(ty: &str, v: &Value) -> bool {
    let p = &v["predicate"];
    let keys_in =
        |o: &Value, allowed: &[&str]| o.as_object().is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str())));
    match ty {
        "damage_source_properties" => {
            keys_in(p, &["source_entity"])
                && keys_in(&p["source_entity"], &["minecraft:entity_type", "minecraft:components"])
        }
        "entity_properties" => match v["entity"].as_str().unwrap_or("this") {
            "attacker" => keys_in(p, &["minecraft:entity_type"]),
            "this" => keys_in(
                p,
                &[
                    "minecraft:flags",
                    "minecraft:components",
                    "minecraft:type_specific/sheep",
                    "minecraft:type_specific/cube_mob",
                    "minecraft:type_specific/raider",
                    "minecraft:type_specific/fishing_hook",
                    "minecraft:entity_type",
                ],
            ),
            _ => false,
        },
        "location_check" => {
            keys_in(p, &["biomes"])
                && v.get("offsetX").is_none()
                && v.get("offsetY").is_none()
                && v.get("offsetZ").is_none()
        }
        _ => false,
    }
}

/// W8c: direct entity of a damage type carried by an entity: the author itself (melee
/// hit, thorns, guardian wave), the projectile of the type (`Some(type)`), or unknown.
pub fn direct_of(ty: &str) -> Option<Option<&'static str>> {
    Some(match ty {
        "minecraft:player_attack"
        | "minecraft:mob_attack"
        | "minecraft:mob_attack_no_aggro"
        | "minecraft:sting"
        | "minecraft:mace_smash"
        | "minecraft:spear"
        | "minecraft:thorns"
        | "minecraft:sonic_boom" => None,
        "minecraft:fireball" => Some("minecraft:fireball"),
        "minecraft:arrow" => Some("minecraft:arrow"),
        "minecraft:trident" => Some("minecraft:trident"),
        "minecraft:wither_skull" => Some("minecraft:wither_skull"),
        "minecraft:wind_charge" => Some("minecraft:wind_charge"),
        "minecraft:fireworks" => Some("minecraft:firework_rocket"),
        "minecraft:spit" => Some("minecraft:llama_spit"),
        _ => return None,
    })
}

/// Stack thrown by `discard` (the game's single empty stack).
const DISCARDED: &str = "minecraft:air";

fn ench_name(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

impl Loot<'_> {
    fn tool_level(&self, ctx: &Ctx<'_>, ench: &str) -> i32 {
        match ctx.tool {
            Some(Some(t)) => self.items.enchantment(t, ench),
            _ => 0,
        }
    }

    /// Condition (inline form, reference to a named predicate, or list = all of them).
    pub fn cond(&self, v: &Value, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> bool {
        match v {
            Value::Null => true,
            Value::Array(a) => a.iter().all(|c| self.cond(c, ctx, rng)),
            Value::String(id) => match self.pack.get("predicate", id) {
                Some(p) => self.cond(&p, ctx, rng),
                None => false,
            },
            Value::Object(o) => {
                let ty = o.get("type").and_then(Value::as_str).unwrap_or("").trim_start_matches("minecraft:");
                match ty {
                    "inverted" => !self.cond(&v["term"], ctx, rng),
                    "any_of" => v["terms"].as_array().is_some_and(|t| t.iter().any(|c| self.cond(c, ctx, rng))),
                    "all_of" => v["terms"].as_array().is_none_or(|t| t.iter().all(|c| self.cond(c, ctx, rng))),
                    "random_chance" => {
                        let p = self.float(&v["chance"], ctx, rng);
                        rng.next_float() < p
                    }
                    "survives_explosion" => match ctx.explosion {
                        Some(r) => rng.next_float() <= 1.0 / r,
                        None => true,
                    },
                    "table_bonus" => {
                        let lvl = self.tool_level(ctx, ench_name(&v["enchantment"]));
                        let chances: Vec<f32> = v["chances"]
                            .as_array()
                            .map(|a| a.iter().filter_map(Value::as_f64).map(|f| f as f32).collect())
                            .unwrap_or_default();
                        let Some(&c) = chances.get((lvl as usize).min(chances.len().saturating_sub(1))) else {
                            return false;
                        };
                        rng.next_float() < c
                    }
                    "match_tool" => match ctx.tool {
                        Some(t) => self.item_pred(&v["predicate"], t),
                        None => false,
                    },
                    "match_block" => self.match_block(v, ctx),
                    "killed_by_player" => ctx.killed_by_player,
                    "random_chance_with_enchanted_bonus" => {
                        let lvl = ctx.looting;
                        let c = if lvl > 0 {
                            let e = &v["enchanted_chance"];
                            match e.as_f64() {
                                Some(x) => x as f32,
                                None => {
                                    let base = e["base"].as_f64().unwrap_or(0.0) as f32;
                                    let per = e["per_level_above_first"].as_f64().unwrap_or(0.0) as f32;
                                    base + per * (lvl - 1) as f32
                                }
                            }
                        } else {
                            v["unenchanted_chance"].as_f64().unwrap_or(0.0) as f32
                        };
                        rng.next_float() < c
                    }
                    // F10: source entity of the damage (exact type, components).
                    // W8c: draw-free conditions entrusted to the world when it is lent.
                    "damage_source_properties" | "entity_properties" | "location_check"
                        if let Some(w) = ctx.world
                            && !locally_known(ty, v) =>
                    {
                        // Non-living direct entity (fireball…): its type tested here, the
                        // rest of the predicate by the world.
                        if ty == "damage_source_properties"
                            && let Some(dt) = &ctx.direct_type
                            && let Some(de) = v["predicate"].get("direct_entity")
                            && de.as_object().is_some_and(|o| o.keys().all(|k| k == "minecraft:entity_type"))
                        {
                            let ok = match &de["minecraft:entity_type"] {
                                Value::String(s) => s == dt,
                                Value::Array(a) => a.iter().any(|x| x.as_str() == Some(dt.as_str())),
                                _ => false,
                            };
                            let mut rest = v.clone();
                            if let Some(o) = rest["predicate"].as_object_mut() {
                                o.remove("direct_entity");
                            }
                            return ok && w(&rest);
                        }
                        w(v)
                    }
                    "damage_source_properties" => {
                        let p = &v["predicate"];
                        let only = p.as_object().is_some_and(|o| o.keys().all(|k| k == "source_entity"));
                        let e = &p["source_entity"];
                        let keys_ok = e.as_object().is_some_and(|o| {
                            o.keys().all(|k| k == "minecraft:entity_type" || k == "minecraft:components")
                        });
                        only && keys_ok
                            && ctx.source_entity.as_ref().is_some_and(|(ty, comps)| {
                                e["minecraft:entity_type"].as_str().is_none_or(|t| t == ty)
                                    && e["minecraft:components"].as_object().is_none_or(|o| {
                                        o.iter().all(|(k, want)| {
                                            comps.iter().any(|(ck, cv)| ck == k && want.as_str() == Some(cv.as_str()))
                                        })
                                    })
                            })
                    }
                    "entity_properties" if v["entity"].as_str() == Some("attacker") => {
                        // Author of the damage: only its type is known.
                        let p = &v["predicate"];
                        let only_type = p.as_object().is_some_and(|o| o.keys().all(|k| k == "minecraft:entity_type"));
                        only_type
                            && ctx.attacker.as_ref().is_some_and(|(name, tags)| match &p["minecraft:entity_type"] {
                                Value::String(t) => match t.strip_prefix('#') {
                                    Some(tag) => tags.iter().any(|x| x == tag),
                                    None => t == name,
                                },
                                Value::Array(a) => a.iter().any(|x| x.as_str() == Some(name.as_str())),
                                _ => true,
                            })
                    }
                    "entity_properties" => {
                        // Only the killed entity's flags are known (fire, baby).
                        let p = &v["predicate"];
                        let this = v["entity"].as_str() == Some("this");
                        let known = [
                            "minecraft:flags",
                            "minecraft:components",
                            "minecraft:type_specific/sheep",
                            "minecraft:type_specific/cube_mob",
                            "minecraft:type_specific/raider",
                            "minecraft:type_specific/fishing_hook",
                            "minecraft:entity_type",
                        ];
                        let only_known = p.as_object().is_some_and(|o| o.keys().all(|k| known.contains(&k.as_str())));
                        this && only_known && {
                            let fl = &p["minecraft:flags"];
                            let comps = &p["minecraft:components"];
                            let color_ok = comps["minecraft:sheep/color"]
                                .as_str()
                                .is_none_or(|c| ctx.sheep.as_ref().is_some_and(|s| s.0 == c));
                            let sheared_ok = p["minecraft:type_specific/sheep"]["sheared"]
                                .as_bool()
                                .is_none_or(|b| ctx.sheep.as_ref().is_some_and(|s| s.1 == b));
                            let comps_ok = comps.as_object().is_none_or(|o| {
                                o.iter().all(|(k, want)| {
                                    k == "minecraft:sheep/color"
                                        || ctx
                                            .comps
                                            .iter()
                                            .any(|(ck, cv)| ck == k && want.as_str() == Some(cv.as_str()))
                                })
                            });
                            let cube = &p["minecraft:type_specific/cube_mob"];
                            let cube_ok = cube.is_null() || ctx.cube_size.is_some_and(|s| int_bounds(&cube["size"], s));
                            let raider = &p["minecraft:type_specific/raider"];
                            // Absent fields = false (not "don't care"), like the game.
                            let raider_ok = raider.is_null()
                                || ctx.raider.is_some_and(|(has, cap)| {
                                    raider["has_raid"].as_bool().unwrap_or(false) == has
                                        && raider["is_captain"].as_bool().unwrap_or(false) == cap
                                });
                            // S8: fishing bobber (open water); without `in_open_water`, true
                            // for any entity.
                            let hook = &p["minecraft:type_specific/fishing_hook"];
                            let hook_ok = hook["in_open_water"].as_bool().is_none_or(|w| ctx.open_water == Some(w));
                            // Exact type or list (entity tags are not known here).
                            let ty_ok = match &p["minecraft:entity_type"] {
                                Value::Null => true,
                                Value::String(s) => ctx.this_type.as_deref() == Some(s.as_str()),
                                Value::Array(a) => {
                                    ctx.this_type.as_deref().is_some_and(|t| a.iter().any(|x| x.as_str() == Some(t)))
                                }
                                _ => false,
                            };
                            raider_ok
                                && ty_ok
                                && hook_ok
                                && fl["is_on_fire"].as_bool().is_none_or(|b| b == ctx.on_fire)
                                && fl["is_baby"].as_bool().is_none_or(|b| b == ctx.baby)
                                && color_ok
                                && sheared_ok
                                && comps_ok
                                && cube_ok
                        }
                    }
                    // S8: location limited to biomes (fishing loot).
                    "location_check" => {
                        let p = &v["predicate"];
                        let only_biomes = p.as_object().is_some_and(|o| o.keys().all(|k| k == "biomes"));
                        only_biomes
                            && ctx.biome.as_deref().is_some_and(|b| match &p["biomes"] {
                                Value::String(s) => s == b,
                                Value::Array(a) => a.iter().any(|x| x.as_str() == Some(b)),
                                _ => false,
                            })
                    }
                    // W8c: other draw-free conditions (weather, time, scores…): the world.
                    _ => ctx.world.is_some_and(|w| w(v)),
                }
            }
            _ => false,
        }
    }

    fn match_block(&self, v: &Value, ctx: &Ctx<'_>) -> bool {
        let Some((name, props)) = &ctx.block else { return false };
        let in_set = |b: &Value| match b {
            Value::String(s) => match s.strip_prefix('#') {
                Some(tag) => (self.block_tag)(tag, name),
                None => s == name,
            },
            Value::Array(a) => a.iter().any(|x| x.as_str() == Some(name)),
            _ => true,
        };
        if !v["blocks"].is_null() && !in_set(&v["blocks"]) {
            return false;
        }
        if let Some(state) = v["state"].as_object() {
            for (k, want) in state {
                let Some((_, have)) = props.iter().find(|(p, _)| p == k) else { return false };
                let ok = match want {
                    Value::String(s) => s == have,
                    Value::Object(r) => {
                        let num =
                            |x: &Value| x.as_str().map(str::to_owned).or_else(|| x.as_i64().map(|i| i.to_string()));
                        let h: i64 = have.parse().unwrap_or(i64::MIN);
                        r.get("min").and_then(num).is_none_or(|m| m.parse::<i64>().is_ok_and(|m| h >= m))
                            && r.get("max").and_then(num).is_none_or(|m| m.parse::<i64>().is_ok_and(|m| h <= m))
                    }
                    other => other.as_i64().map(|i| i.to_string()).as_deref() == Some(have.as_str()),
                };
                if !ok {
                    return false;
                }
            }
        }
        true
    }

    /// Item predicate: `items`, `count`, `enchantments` component predicates.
    pub fn item_pred(&self, p: &Value, s: Option<&Stack>) -> bool {
        let Some(s) = s else { return p.is_null() };
        if let Some(items) = p.get("items") {
            let ok = match items {
                Value::String(x) => match x.strip_prefix('#') {
                    Some(tag) => self.items.has_tag(tag, &s.id),
                    None => *x == s.id,
                },
                Value::Array(a) => a.iter().any(|x| x.as_str() == Some(s.id.as_str())),
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        if let Some(c) = p.get("count") {
            let n = i64::from(s.count);
            let ok = match c {
                Value::Number(x) => x.as_i64() == Some(n),
                Value::Object(r) => {
                    r.get("min").and_then(Value::as_i64).is_none_or(|m| n >= m)
                        && r.get("max").and_then(Value::as_i64).is_none_or(|m| n <= m)
                }
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        if let Some(list) = p.get("predicates").and_then(|x| x.get("minecraft:enchantments")).and_then(Value::as_array)
        {
            for e in list {
                let ids: Vec<String> = match &e["enchantments"] {
                    Value::String(x) => vec![x.clone()],
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                    _ => Vec::new(),
                };
                let lvl = ids.iter().map(|id| self.items.enchantment(s, id)).max().unwrap_or(0);
                let (min, max) = match &e["levels"] {
                    Value::Number(n) => (n.as_i64().unwrap_or(1), n.as_i64().unwrap_or(1)),
                    Value::Object(r) => (
                        r.get("min").and_then(Value::as_i64).unwrap_or(1),
                        r.get("max").and_then(Value::as_i64).unwrap_or(i64::MAX),
                    ),
                    _ => (1, i64::MAX),
                };
                if lvl == 0 || i64::from(lvl) < min || i64::from(lvl) > max {
                    return false;
                }
            }
        }
        true
    }

    /// Integer provider (`context_int_provider`): 0 if a computation overflows or makes no
    /// sense (division by zero, impossible conversion), like reading an integer.
    pub fn int(&self, v: &Value, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> i32 {
        self.int_unsafe(v, ctx, rng).unwrap_or(0)
    }

    /// Float provider (`context_float_provider`): 0 if the computation fails or is not
    /// finite, like reading a float.
    pub fn float(&self, v: &Value, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> f32 {
        self.float_unsafe(v, ctx, rng).filter(|f| f.is_finite()).unwrap_or(0.0)
    }

    /// Entries of an aggregate provider (`inputs`: list, reference or tag of providers).
    fn inputs(&self, v: &Value, reg: &str) -> Vec<Value> {
        match v {
            Value::Array(a) => a.clone(),
            Value::String(id) => match id.strip_prefix('#') {
                Some(_) => Vec::new(),
                None => self.pack.get(reg, id).map(|a| (*a).clone()).into_iter().collect(),
            },
            Value::Null => Vec::new(),
            o => vec![o.clone()],
        }
    }

    /// Value drawn from a weighted distribution (`distribution`: `data`, `weight`).
    fn weighted<'v>(&self, v: &'v Value, rng: &mut dyn Rand) -> Option<&'v Value> {
        let list = v["distribution"].as_array()?;
        let total: i64 = list.iter().map(|e| e["weight"].as_i64().unwrap_or(0)).sum();
        if total <= 0 || total > i64::from(i32::MAX) {
            return None;
        }
        let mut k = i64::from(rng.next_int_bound(total as i32));
        for e in list {
            k -= e["weight"].as_i64().unwrap_or(0);
            if k < 0 {
                return Some(&e["data"]);
            }
        }
        None
    }

    /// Case chosen by a switch (`cases`: `condition`, `value`; otherwise `default`).
    fn dispatch<'v>(&self, v: &'v Value, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> Option<&'v Value> {
        for c in v["cases"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            if self.cond(&c["condition"], ctx, rng) {
                return Some(&c["value"]);
            }
        }
        v.get("default")
    }

    /// `None` = arithmetic error (propagated up to the first call).
    fn int_unsafe(&self, v: &Value, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> Option<i32> {
        let int_of = |f: f32| -> Option<i32> {
            if !f.is_finite() {
                return None;
            }
            let l = f as i64;
            i32::try_from(l).ok()
        };
        match v {
            Value::Number(n) => Some(n.as_i64().map_or_else(|| n.as_f64().unwrap_or(0.0) as i32, |i| i as i32)),
            Value::String(id) => match self.pack.get("context_int_provider", id) {
                Some(p) => self.int_unsafe(&p, ctx, rng),
                None => Some(0),
            },
            Value::Object(o) => {
                let ty = o.get("type").and_then(Value::as_str).unwrap_or("").trim_start_matches("minecraft:");
                let reg = "context_int_provider";
                let all = |s: &Self, rng: &mut dyn Rand| -> Option<Vec<i32>> {
                    s.inputs(&v["inputs"], reg).iter().map(|x| s.int_unsafe(x, ctx, rng)).collect()
                };
                let two = |s: &Self, rng: &mut dyn Rand| -> Option<(i32, i32)> {
                    let a = s.int_unsafe(&v["left"], ctx, rng)?;
                    let b = s.int_unsafe(&v["right"], ctx, rng)?;
                    Some((a, b))
                };
                let one = |s: &Self, rng: &mut dyn Rand| s.int_unsafe(&v["input"], ctx, rng);
                match ty {
                    "constant" => v["value"].as_i64().map(|x| x as i32).or(Some(0)),
                    "uniform" => {
                        let a = self.int_unsafe(&v["min"], ctx, rng)?;
                        let b = self.int_unsafe(&v["max"], ctx, rng)?;
                        Some(rng.between(a, b))
                    }
                    "binomial" => {
                        let n = self.int_unsafe(&v["n"], ctx, rng)?;
                        let p = self.float_unsafe(&v["p"], ctx, rng)?;
                        Some((0..n).filter(|_| rng.next_float() < p).count() as i32)
                    }
                    "conditional" => {
                        if self.cond(&v["condition"], ctx, rng) {
                            self.int_unsafe(&v["on_true"], ctx, rng)
                        } else {
                            match v.get("on_false") {
                                Some(f) => self.int_unsafe(f, ctx, rng),
                                None => Some(0),
                            }
                        }
                    }
                    "number_dispatcher" => match self.dispatch(v, ctx, rng) {
                        Some(x) => self.int_unsafe(x, ctx, rng),
                        None => Some(0),
                    },
                    "weighted_list" => {
                        let x = self.weighted(v, rng)?;
                        self.int_unsafe(x, ctx, rng)
                    }
                    "add" => i32::try_from(all(self, rng)?.iter().map(|&x| i64::from(x)).sum::<i64>()).ok(),
                    "mul" => {
                        let mut p = 1i64;
                        for x in all(self, rng)? {
                            p = p.wrapping_mul(i64::from(x));
                        }
                        i32::try_from(p).ok()
                    }
                    "min" => Some(all(self, rng)?.into_iter().fold(i32::MAX, i32::min)),
                    "max" => Some(all(self, rng)?.into_iter().fold(i32::MIN, i32::max)),
                    "avg" => {
                        let xs = all(self, rng)?;
                        if xs.is_empty() {
                            return None;
                        }
                        let sum: i64 = xs.iter().map(|&x| i64::from(x)).sum();
                        i32::try_from(sum / xs.len() as i64).ok()
                    }
                    "sub" => {
                        let (a, b) = two(self, rng)?;
                        a.checked_sub(b)
                    }
                    "div" => {
                        let (a, b) = two(self, rng)?;
                        a.checked_div(b)
                    }
                    "mod" => {
                        let (a, b) = two(self, rng)?;
                        a.checked_rem(b)
                    }
                    "floor_div" => {
                        let (a, b) = two(self, rng)?;
                        if b == 0 || (a == i32::MIN && b == -1) {
                            return None;
                        }
                        let q = a / b;
                        Some(if a % b != 0 && ((a ^ b) < 0) { q - 1 } else { q })
                    }
                    "floor_mod" => {
                        let (a, b) = two(self, rng)?;
                        if b == 0 {
                            return None;
                        }
                        let r = a.wrapping_rem(b);
                        Some(if r != 0 && ((r ^ b) < 0) { r + b } else { r })
                    }
                    "abs" => one(self, rng)?.checked_abs(),
                    "negate" => one(self, rng)?.checked_neg(),
                    "pow" => {
                        let base = self.int_unsafe(&v["base"], ctx, rng)?;
                        let e = self.int_unsafe(&v["exponent"], ctx, rng)?;
                        if (base == 0 && e == 0) || e < 0 {
                            return None;
                        }
                        base.checked_pow(e as u32)
                    }
                    "from_float" => int_of(self.float_unsafe(&v["input"], ctx, rng)?),
                    // Scoreboard, storage, environment attributes: world not lent,
                    // fallback value.
                    "score" | "storage" => match v.get("fallback") {
                        Some(f) => self.int_unsafe(f, ctx, rng),
                        None => Some(0),
                    },
                    _ => Some(0),
                }
            }
            _ => Some(0),
        }
    }

    /// `None` = arithmetic error.
    fn float_unsafe(&self, v: &Value, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> Option<f32> {
        match v {
            Value::Number(n) => Some(n.as_f64().unwrap_or(0.0) as f32),
            Value::String(id) => match self.pack.get("context_float_provider", id) {
                Some(p) => self.float_unsafe(&p, ctx, rng),
                None => Some(0.0),
            },
            Value::Object(o) => {
                let ty = o.get("type").and_then(Value::as_str).unwrap_or("").trim_start_matches("minecraft:");
                let reg = "context_float_provider";
                let all = |s: &Self, rng: &mut dyn Rand| -> Option<Vec<f32>> {
                    s.inputs(&v["inputs"], reg).iter().map(|x| s.float_unsafe(x, ctx, rng)).collect()
                };
                let two = |s: &Self, rng: &mut dyn Rand| -> Option<(f32, f32)> {
                    let a = s.float_unsafe(&v["left"], ctx, rng)?;
                    let b = s.float_unsafe(&v["right"], ctx, rng)?;
                    Some((a, b))
                };
                let one = |s: &Self, rng: &mut dyn Rand| s.float_unsafe(&v["input"], ctx, rng);
                Some(match ty {
                    "constant" => v["value"].as_f64().unwrap_or(0.0) as f32,
                    "uniform" => {
                        let a = self.float_unsafe(&v["min"], ctx, rng)?;
                        let b = self.float_unsafe(&v["max"], ctx, rng)?;
                        if a >= b { a } else { rng.next_float() * (b - a) + a }
                    }
                    "conditional" => {
                        return if self.cond(&v["condition"], ctx, rng) {
                            self.float_unsafe(&v["on_true"], ctx, rng)
                        } else {
                            match v.get("on_false") {
                                Some(f) => self.float_unsafe(f, ctx, rng),
                                None => Some(0.0),
                            }
                        };
                    }
                    "number_dispatcher" => {
                        return match self.dispatch(v, ctx, rng) {
                            Some(x) => self.float_unsafe(x, ctx, rng),
                            None => Some(0.0),
                        };
                    }
                    "weighted_list" => {
                        let x = self.weighted(v, rng)?;
                        return self.float_unsafe(x, ctx, rng);
                    }
                    "add" => all(self, rng)?.into_iter().fold(0.0, |a, x| a + x),
                    "mul" => all(self, rng)?.into_iter().fold(1.0, |a, x| a * x),
                    "min" => all(self, rng)?.into_iter().fold(f32::MAX, f32::min),
                    "max" => all(self, rng)?.into_iter().fold(-f32::MAX, f32::max),
                    "avg" => {
                        let xs = all(self, rng)?;
                        xs.iter().fold(0.0f32, |a, x| a + x) / xs.len() as f32
                    }
                    "length" => all(self, rng)?.into_iter().fold(0.0f32, |a, x| a + x * x).sqrt(),
                    "sub" => {
                        let (a, b) = two(self, rng)?;
                        a - b
                    }
                    "div" => {
                        let (a, b) = two(self, rng)?;
                        a / b
                    }
                    "mod" => {
                        let (a, b) = two(self, rng)?;
                        if b == 0.0 { f32::NAN } else { a % b }
                    }
                    "pow" => {
                        let base = self.float_unsafe(&v["base"], ctx, rng)?;
                        let e = self.float_unsafe(&v["exponent"], ctx, rng)?;
                        if base == 0.0 && e == 0.0 { f32::NAN } else { f64::from(base).powf(f64::from(e)) as f32 }
                    }
                    "abs" => one(self, rng)?.abs(),
                    "negate" => -one(self, rng)?,
                    "floor" => one(self, rng)?.floor(),
                    "ceil" => one(self, rng)?.ceil(),
                    "round" => {
                        let x = one(self, rng)?;
                        if x.is_nan() { 0.0 } else { (x + 0.5).floor().clamp(i32::MIN as f32, i32::MAX as f32) }
                    }
                    "truncate" => {
                        let x = one(self, rng)?;
                        if x > 0.0 { x.floor() } else { x.ceil() }
                    }
                    "sqrt" => one(self, rng)?.sqrt(),
                    "sin" => crate::mobs::mth::sin(one(self, rng)?),
                    "cos" => crate::mobs::mth::cos(one(self, rng)?),
                    "from_int" => self.int_unsafe(&v["input"], ctx, rng)? as f32,
                    // Storage: world not lent, fallback value; enchantment level absent
                    // from the loot context: level 0.
                    "storage" => {
                        return match v.get("fallback") {
                            Some(f) => self.float_unsafe(f, ctx, rng),
                            None => Some(0.0),
                        };
                    }
                    _ => 0.0,
                })
            }
            _ => Some(0.0),
        }
    }

    /// Item functions applied to a stack (trades, F05).
    pub(crate) fn apply_fn(&self, f: &Value, s: Stack, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> Stack {
        self.apply(f, s, ctx, rng)
    }

    /// Item functions (one, a list, or an `item_modifier` reference).
    pub(crate) fn apply(&self, f: &Value, mut s: Stack, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> Stack {
        match f {
            Value::Null => s,
            Value::Array(a) => {
                for x in a {
                    s = self.apply(x, s, ctx, rng);
                }
                s
            }
            Value::String(id) => match self.pack.get("item_modifier", id) {
                Some(m) => self.apply(&m, s, ctx, rng),
                None => s,
            },
            Value::Object(o) => {
                if o.contains_key("condition") && !self.cond(&f["condition"], ctx, rng) {
                    return s;
                }
                let ty = o.get("type").and_then(Value::as_str).unwrap_or("").trim_start_matches("minecraft:");
                match ty {
                    "set_count" => {
                        let base = if f["add"].as_bool().unwrap_or(false) { s.count } else { 0 };
                        s.count = base + self.int(&f["count"], ctx, rng);
                    }
                    "apply_bonus" => {
                        if let Some(tool) = ctx.tool {
                            let lvl = tool.map_or(0, |t| self.items.enchantment(t, ench_name(&f["enchantment"])));
                            let formula = f["formula"].as_str().unwrap_or("").trim_start_matches("minecraft:");
                            let params = &f["parameters"];
                            s.count = match formula {
                                "ore_drops" => {
                                    if lvl > 0 {
                                        let bonus = (rng.next_int_bound(lvl + 2) - 1).max(0);
                                        s.count * (bonus + 1)
                                    } else {
                                        s.count
                                    }
                                }
                                "uniform_bonus_count" => {
                                    let m = params["bonusMultiplier"].as_i64().unwrap_or(1) as i32;
                                    s.count + rng.next_int_bound(m * lvl + 1)
                                }
                                "binomial_with_bonus_count" => {
                                    let extra = params["extra"].as_i64().unwrap_or(0) as i32;
                                    let p = params["probability"].as_f64().unwrap_or(0.0) as f32;
                                    let mut c = s.count;
                                    for _ in 0..lvl + extra {
                                        if rng.next_float() < p {
                                            c += 1;
                                        }
                                    }
                                    c
                                }
                                _ => s.count,
                            };
                        }
                    }
                    "explosion_decay" => {
                        if let Some(r) = ctx.explosion {
                            let p = 1.0 / r;
                            s.count = (0..s.count).filter(|_| rng.next_float() <= p).count() as i32;
                        }
                    }
                    "limit_count" => {
                        let lim = &f["limit"];
                        if let Some(min) = lim.get("min").filter(|m| !m.is_null()) {
                            s.count = s.count.max(self.int(min, ctx, rng));
                        }
                        if let Some(max) = lim.get("max").filter(|m| !m.is_null()) {
                            s.count = s.count.min(self.int(max, ctx, rng));
                        }
                    }
                    "copy_components" => {
                        // Block-entity source: components recorded with it (name, patterns…).
                        if f["source"].as_str() == Some("block_entity")
                            && let Some(be) = ctx.block_entity
                        {
                            let include: Vec<&str> = f["include"]
                                .as_array()
                                .map(|a| a.iter().filter_map(Value::as_str).collect())
                                .unwrap_or_default();
                            let short = |n: &str| n.trim_start_matches("minecraft:").to_owned();
                            let exclude: Vec<&str> = f["exclude"]
                                .as_array()
                                .map(|a| a.iter().filter_map(Value::as_str).collect())
                                .unwrap_or_default();
                            // W8c: collected components (contents, lock, patterns, bees…)
                            // then those the block entity keeps as they are.
                            for (k, v) in be::collect(be) {
                                let inc = f.get("include").is_none() || include.iter().any(|i| short(i) == short(&k));
                                if inc && !exclude.iter().any(|i| short(i) == short(&k)) {
                                    crate::items::set_component(&mut s, &k, v);
                                }
                            }
                        }
                    }
                    "copy_state" => {
                        if let Some((_, props)) = &ctx.block {
                            let keep: Vec<&str> = f["properties"]
                                .as_array()
                                .map(|a| a.iter().filter_map(Value::as_str).collect())
                                .unwrap_or_default();
                            let fields: Vec<(String, Nbt)> = props
                                .iter()
                                .filter(|(k, _)| keep.contains(&k.as_str()))
                                .map(|(k, v)| (k.clone(), Nbt::String(v.clone())))
                                .collect();
                            if !fields.is_empty() {
                                crate::items::set_component(&mut s, "minecraft:block_state", Nbt::Compound(fields));
                            }
                        }
                    }
                    "set_components" => {
                        if let Some(c) = f["components"].as_object() {
                            for (k, v) in c {
                                crate::items::set_component(&mut s, k, crate::recipes::json_to_nbt(v));
                            }
                        }
                    }
                    "sequence" => {
                        s = self.apply(&f["functions"], s, ctx, rng);
                    }
                    // Final empty stack: a following function does not revive it.
                    "discard" => {
                        s.id = DISCARDED.to_owned();
                        s.count = 0;
                        s.components = None;
                    }
                    "enchanted_count_increase" => {
                        if ctx.looting > 0 {
                            let k = ctx.looting as f32 * self.float(&f["count"], ctx, rng);
                            s.count += (k + 0.5).floor() as i32;
                            if let Some(l) = f["limit"].as_i64().filter(|l| *l > 0) {
                                s.count = s.count.min(l as i32);
                            }
                        }
                    }
                    // W8c: smelting recipe of the world when it is lent (quantity × input
                    // if `use_input_count`, capped at the result's stack).
                    "furnace_smelt" if ctx.smelt.is_some() => {
                        if let Some(r) = ctx.smelt.and_then(|f| f(&s)).filter(|r| r.count > 0) {
                            let n = if f["use_input_count"].as_bool().unwrap_or(true) { s.count } else { 1 };
                            let max = self.items.max_stack(&r);
                            s = Stack { count: (n.saturating_mul(r.count)).min(max), ..r };
                        }
                    }
                    "furnace_smelt" => {
                        let cooked = match s.id.as_str() {
                            "minecraft:beef" => "minecraft:cooked_beef",
                            "minecraft:porkchop" => "minecraft:cooked_porkchop",
                            "minecraft:chicken" => "minecraft:cooked_chicken",
                            "minecraft:mutton" => "minecraft:cooked_mutton",
                            "minecraft:rabbit" => "minecraft:cooked_rabbit",
                            "minecraft:cod" => "minecraft:cooked_cod",
                            "minecraft:salmon" => "minecraft:cooked_salmon",
                            "minecraft:potato" => "minecraft:baked_potato",
                            _ => "",
                        };
                        if !cooked.is_empty() {
                            s.id = cooked.to_owned();
                        }
                    }
                    // S7: functions of structure chests.
                    _ => s = self.apply_s7(ty, f, s, ctx, rng),
                }
                s
            }
            _ => s,
        }
    }

    /// Expands an entry into candidates (simple entry + weight).
    fn expand<'v>(&self, e: &'v Value, ctx: &Ctx<'_>, rng: &mut dyn Rand, out: &mut Vec<Cand<'v>>) -> bool {
        if !self.cond(&e["condition"], ctx, rng) {
            return false;
        }
        let ty = e["type"].as_str().unwrap_or("").trim_start_matches("minecraft:");
        match ty {
            "alternatives" => {
                for c in e["children"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    if self.expand(c, ctx, rng, out) {
                        return true;
                    }
                }
                false
            }
            "sequence" => {
                for c in e["children"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    if !self.expand(c, ctx, rng, out) {
                        return false;
                    }
                }
                true
            }
            "group" => {
                for c in e["children"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    self.expand(c, ctx, rng, out);
                }
                true
            }
            _ => {
                let weight = e["weight"].as_i64().unwrap_or(1) as f32;
                let quality = e["quality"].as_i64().unwrap_or(0) as f32;
                let w = ((weight + quality * ctx.luck).floor() as i32).max(0);
                if w > 0 {
                    // Expanded tag entry: one candidate per item, each with the weight of
                    // the entry.
                    if ty == "tag" && e["expand"].as_bool() == Some(true) {
                        for id in self.holder_items(&e["items"]) {
                            out.push((e, w, Some(id)));
                        }
                    } else {
                        out.push((e, w, None));
                    }
                }
                true
            }
        }
    }

    /// Items of a set (`#tag`, single id or list), in the tag's order.
    fn holder_items(&self, v: &Value) -> Vec<String> {
        match v {
            Value::String(x) => match x.strip_prefix('#') {
                Some(tag) => self
                    .items
                    .tags
                    .get(tag.trim_start_matches("minecraft:"))
                    .or_else(|| self.items.tags.get(tag))
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|id| self.items.name(id).map(str::to_owned))
                    .collect(),
                None => vec![x.clone()],
            },
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
            _ => Vec::new(),
        }
    }

    fn create(
        &self,
        e: &Value,
        pick: Option<&str>,
        ctx: &Ctx<'_>,
        rng: &mut dyn Rand,
        depth: u32,
        out: &mut Vec<Stack>,
    ) {
        let ty = e["type"].as_str().unwrap_or("").trim_start_matches("minecraft:");
        let mut produced = Vec::new();
        match ty {
            "item" => {
                if let Some(name) = e["name"].as_str() {
                    produced.push(Stack { id: name.to_owned(), count: 1, components: None });
                }
            }
            "tag" => {
                // `items` field (old name: `name`); expanded: the single item drawn.
                let items = if e["items"].is_null() { &e["name"] } else { &e["items"] };
                let items = match items {
                    Value::String(x) if !x.starts_with('#') => Value::String(format!("#{x}")),
                    v => v.clone(),
                };
                for n in self.holder_items(&items) {
                    if pick.is_none_or(|p| p == n) {
                        produced.push(Stack { id: n, count: 1, components: None });
                    }
                }
            }
            "loot_table" if depth < 8 => match &e["value"] {
                Value::String(id) => {
                    if let Some(t) = self.pack.get("loot_table", id) {
                        produced.extend(self.run_raw(&t, ctx, rng, depth + 1));
                    }
                }
                t @ Value::Object(_) => produced.extend(self.run_raw(t, ctx, rng, depth + 1)),
                _ => {}
            },
            "dynamic" => {
                // Contents supplied by the block, by name: `sherds` (decorated pot: sherds
                // left, back, front, right; brick = empty face), `contents` (Shulker
                // box: slots in order).
                if let Some(be) = ctx.block_entity {
                    match e["name"].as_str().map(|n| n.trim_start_matches("minecraft:")) {
                        Some("sherds") => {
                            // Disc: back, left, right, front.
                            let s = crate::nbtio::list(be.get("sherds"));
                            for k in [1usize, 0, 3, 2] {
                                if let Some(id) = s.get(k).and_then(Nbt::as_str).filter(|id| *id != "minecraft:brick") {
                                    produced.push(Stack { id: id.to_owned(), count: 1, components: None });
                                }
                            }
                        }
                        Some("contents") => {
                            for it in crate::nbtio::list(be.get("Items")) {
                                if let Some(st) = Stack::from_nbt(it) {
                                    produced.push(st);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        // Stack emptied by the entry's functions: it still goes through those of the
        // pool and the table (the game only sorts out empty stacks at the end).
        for s in produced {
            out.push(self.apply(&e["modifier"], s, ctx, rng));
        }
    }

    /// Runs a table: items produced (not yet split into max-size stacks), empty
    /// stacks removed.
    pub fn run(&self, table: &Value, ctx: &Ctx<'_>, rng: &mut dyn Rand, depth: u32) -> Vec<Stack> {
        let mut out = self.run_raw(table, ctx, rng, depth);
        out.retain(|s| s.count > 0 && s.id != DISCARDED);
        out
    }

    /// Raw table: empty stacks included, as received by a `loot_table` entry
    /// that nests it.
    fn run_raw(&self, table: &Value, ctx: &Ctx<'_>, rng: &mut dyn Rand, depth: u32) -> Vec<Stack> {
        let mut out = Vec::new();
        for pool in table["pools"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            if !self.cond(&pool["condition"], ctx, rng) {
                continue;
            }
            let rolls = self.int(&pool["rolls"], ctx, rng)
                + (self.float(&pool["bonus_rolls"], ctx, rng) * ctx.luck).floor() as i32;
            for _ in 0..rolls {
                let mut cands: Vec<Cand<'_>> = Vec::new();
                for e in pool["entries"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    self.expand(e, ctx, rng, &mut cands);
                }
                let total: i32 = cands.iter().map(|c| c.1).sum();
                if total == 0 || cands.is_empty() {
                    continue;
                }
                let chosen = if cands.len() == 1 {
                    Some(&cands[0])
                } else {
                    let mut r = rng.next_int_bound(total);
                    cands.iter().find(|(_, w, _)| {
                        r -= w;
                        r < 0
                    })
                };
                let Some((e, _, pick)) = chosen else { continue };
                let mut made = Vec::new();
                self.create(e, pick.as_deref(), ctx, rng, depth, &mut made);
                for s in made {
                    let s = self.apply(&pool["modifier"], s, ctx, rng);
                    out.push(self.apply(&table["modifier"], s, ctx, rng));
                }
            }
        }
        out
    }

    /// Named table, stacks split to their maximum size.
    pub fn drops(&self, id: &str, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> Vec<Stack> {
        let Some(t) = self.pack.get("loot_table", id) else { return Vec::new() };
        let mut out = Vec::new();
        for mut s in self.run(&t, ctx, rng, 0) {
            let max = self.items.max_stack(&s);
            while s.count > max {
                out.push(Stack { count: max, ..s.clone() });
                s.count -= max;
            }
            if s.count > 0 {
                out.push(s);
            }
        }
        out
    }
}

/// F03: integer bounds of a predicate (exact number or `{min, max}`).
fn int_bounds(v: &Value, x: i32) -> bool {
    match v {
        Value::Null => true,
        Value::Number(n) => n.as_i64() == Some(i64::from(x)),
        _ => {
            let x = i64::from(x);
            v["min"].as_i64().is_none_or(|m| x >= m) && v["max"].as_i64().is_none_or(|m| x <= m)
        }
    }
}

// ---------- W8c: components of a block entity (collecting the components / applying the components) ----------

#[path = "loot_be.rs"]
pub mod be;

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Ctx, Loot};

    #[test]
    fn emptied_stack_still_goes_through_pool_and_table_functions() {
        // Stack set to 0 by its entry: the pool's function (adding 2) revives it,
        // like the game, which only sorts out empty stacks at the end; `discard` stays final.
        let Some((s, dir)) = crate::portal_tests::open_new_world("loot0", 7) else { return };
        let no_tag = |_: &str, _: &str| false;
        let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &no_tag };
        let ctx = Ctx::default();
        let mut rng = crate::mobs::jrand::JRand::new(1);
        let table = |entry_fn: serde_json::Value| {
            json!({"pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:stone", "modifier": entry_fn}],
                "modifier": {"type": "minecraft:set_count", "count": 2, "add": true}}]})
        };
        let out = loot.run(&table(json!({"type": "minecraft:set_count", "count": 0})), &ctx, &mut rng, 0);
        assert_eq!(out.iter().map(|x| (x.id.as_str(), x.count)).collect::<Vec<_>>(), [("minecraft:stone", 2)]);
        let out = loot.run(&table(json!({"type": "minecraft:discard"})), &ctx, &mut rng, 0);
        assert!(out.is_empty(), "{out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G1-F07: `type_specific/raider` reads its absent fields as false (old: don't care);
    /// `type_specific/fishing_hook` without `in_open_water` is true for any entity (old: false).
    #[test]
    fn raider_and_hook_entity_conditions_defaults() {
        let data = std::sync::Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
        let dir = std::env::temp_dir().join(format!("rc-loot-g1f07-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = crate::newworld::NewWorld::new("g1f07");
        w.preset = crate::newworld::Preset::Flat;
        crate::newworld::create_world(&dir, &data, &w).expect("world creation");
        let p = crate::blocks::probe::Probe::open(&dir, data);
        let s = &p.s;
        let no_tag = |_: &str, _: &str| false;
        let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &no_tag };
        let mut rng = crate::mobs::jrand::JRand::new(1);
        let cond = |pred: serde_json::Value| json!({"type": "minecraft:entity_properties", "entity": "this", "predicate": pred});
        let raider = cond(json!({"minecraft:type_specific/raider": {}}));
        let in_raid = Ctx { raider: Some((true, false)), ..Ctx::default() };
        let calm = Ctx { raider: Some((false, false)), ..Ctx::default() };
        let mut fails = Vec::new();
        if loot.cond(&raider, &in_raid, &mut rng) || !loot.cond(&raider, &calm, &mut rng) {
            fails.push("raider: has_raid absent = false");
        }
        let hook = cond(json!({"minecraft:type_specific/fishing_hook": {}}));
        let open = cond(json!({"minecraft:type_specific/fishing_hook": {"in_open_water": true}}));
        if !loot.cond(&hook, &Ctx::default(), &mut rng) || loot.cond(&open, &Ctx::default(), &mut rng) {
            fails.push("bobber without field = true");
        }
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(fails.is_empty(), "{fails:?}");
    }
}
