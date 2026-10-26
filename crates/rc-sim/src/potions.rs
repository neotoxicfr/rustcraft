//! Potions (S7, `mechanics-s7.md` § 2): effects of each potion of the registry, contents of a
//! stack (`potion_contents`: potion, added effects, duration scale), effects on consumption
//! (drunk potion, suspicious stew, ominous bottle, `on_consume_effects` of the item reports).

use rc_proto::nbt::Nbt;

use crate::effects::Inst;
use crate::items::component;
use crate::player::Stack;
use crate::server::Server;

/// Potion → effects (effect, duration in ticks, amplifier).
pub fn potion_effects(potion: &str) -> &'static [(&'static str, i32, i32)] {
    match potion.trim_start_matches("minecraft:") {
        "night_vision" => &[("minecraft:night_vision", 3600, 0)],
        "long_night_vision" => &[("minecraft:night_vision", 9600, 0)],
        "invisibility" => &[("minecraft:invisibility", 3600, 0)],
        "long_invisibility" => &[("minecraft:invisibility", 9600, 0)],
        "leaping" => &[("minecraft:jump_boost", 3600, 0)],
        "long_leaping" => &[("minecraft:jump_boost", 9600, 0)],
        "strong_leaping" => &[("minecraft:jump_boost", 1800, 1)],
        "fire_resistance" => &[("minecraft:fire_resistance", 3600, 0)],
        "long_fire_resistance" => &[("minecraft:fire_resistance", 9600, 0)],
        "swiftness" => &[("minecraft:speed", 3600, 0)],
        "long_swiftness" => &[("minecraft:speed", 9600, 0)],
        "strong_swiftness" => &[("minecraft:speed", 1800, 1)],
        "slowness" => &[("minecraft:slowness", 1800, 0)],
        "long_slowness" => &[("minecraft:slowness", 4800, 0)],
        "strong_slowness" => &[("minecraft:slowness", 400, 3)],
        "turtle_master" => &[("minecraft:slowness", 400, 3), ("minecraft:resistance", 400, 2)],
        "long_turtle_master" => &[("minecraft:slowness", 800, 3), ("minecraft:resistance", 800, 2)],
        "strong_turtle_master" => &[("minecraft:slowness", 400, 5), ("minecraft:resistance", 400, 3)],
        "water_breathing" => &[("minecraft:water_breathing", 3600, 0)],
        "long_water_breathing" => &[("minecraft:water_breathing", 9600, 0)],
        "healing" => &[("minecraft:instant_health", 1, 0)],
        "strong_healing" => &[("minecraft:instant_health", 1, 1)],
        "harming" => &[("minecraft:instant_damage", 1, 0)],
        "strong_harming" => &[("minecraft:instant_damage", 1, 1)],
        "poison" => &[("minecraft:poison", 900, 0)],
        "long_poison" => &[("minecraft:poison", 1800, 0)],
        "strong_poison" => &[("minecraft:poison", 432, 1)],
        "regeneration" => &[("minecraft:regeneration", 900, 0)],
        "long_regeneration" => &[("minecraft:regeneration", 1800, 0)],
        "strong_regeneration" => &[("minecraft:regeneration", 450, 1)],
        "strength" => &[("minecraft:strength", 3600, 0)],
        "long_strength" => &[("minecraft:strength", 9600, 0)],
        "strong_strength" => &[("minecraft:strength", 1800, 1)],
        "weakness" => &[("minecraft:weakness", 1800, 0)],
        "long_weakness" => &[("minecraft:weakness", 4800, 0)],
        "luck" => &[("minecraft:luck", 6000, 0)],
        "slow_falling" => &[("minecraft:slow_falling", 1800, 0)],
        "long_slow_falling" => &[("minecraft:slow_falling", 4800, 0)],
        "wind_charged" => &[("minecraft:wind_charged", 3600, 0)],
        "weaving" => &[("minecraft:weaving", 3600, 0)],
        "oozing" => &[("minecraft:oozing", 3600, 0)],
        "infested" => &[("minecraft:infested", 3600, 0)],
        _ => &[],
    }
}

/// Potion carried by a stack (`potion_contents`: string or compound).
pub fn potion_of(s: &Stack) -> Option<String> {
    match component(s, "minecraft:potion_contents")? {
        Nbt::String(p) => Some(p.clone()),
        c => c.get("potion").and_then(Nbt::as_str).map(str::to_owned),
    }
}

/// All effects of a stack (potion then added effects), durations scaled.
pub fn stack_effects(s: &Stack, scale: f32) -> Vec<Inst> {
    match component(s, "minecraft:potion_contents") {
        Some(c) => contents_effects(c).into_iter().map(|i| i.scaled(scale)).collect(),
        None => Vec::new(),
    }
}

/// Effects of a `potion_contents` component (string or compound): potion then added effects,
/// durations as is.
pub fn contents_effects(c: &Nbt) -> Vec<Inst> {
    let potion = match c {
        Nbt::String(p) => Some(p.as_str()),
        c => c.get("potion").and_then(Nbt::as_str),
    };
    let mut out: Vec<Inst> =
        potion.map(potion_effects).unwrap_or(&[]).iter().map(|(id, dur, amp)| Inst::new(id, *dur, *amp)).collect();
    out.extend(crate::nbtio::list(c.get("custom_effects")).iter().filter_map(Inst::from_nbt));
    out
}

impl Server {
    /// Effects of consuming a stack, after food: "listening" components
    /// (potion, suspicious stew, ominous bottle) then `on_consume_effects`.
    pub(crate) fn consume_effects(&mut self, s: &Stack) {
        // Drunk potion.
        if component(s, "minecraft:potion_contents").is_some() {
            let scale = component(s, "minecraft:potion_duration_scale")
                .and_then(|v| match v {
                    Nbt::Float(f) => Some(*f),
                    Nbt::Double(d) => Some(*d as f32),
                    _ => None,
                })
                .or_else(|| {
                    self.life.items.default_component(s, "minecraft:potion_duration_scale")?.as_f64().map(|f| f as f32)
                })
                .unwrap_or(1.0);
            for e in stack_effects(s, scale) {
                if crate::lootfn::instantaneous(&e.id) {
                    self.apply_instant(&e.id, e.amp, 1.0);
                } else {
                    self.add_effect(e);
                }
            }
        }
        // Suspicious stew.
        if let Some(Nbt::List(l)) = component(s, "minecraft:suspicious_stew_effects") {
            for e in l.clone() {
                let (Some(id), d) = (e.get("id").and_then(Nbt::as_str), e.get("duration").and_then(Nbt::as_i64)) else {
                    continue;
                };
                self.add_effect(Inst::new(id, d.unwrap_or(160) as i32, 0));
            }
        }
        // Ominous bottle.
        if let Some(a) = component(s, "minecraft:ominous_bottle_amplifier").and_then(Nbt::as_i64).or_else(|| {
            self.life.items.default_component(s, "minecraft:ominous_bottle_amplifier").and_then(|v| v.as_i64())
        }) {
            let mut e = Inst::new("minecraft:bad_omen", 120000, a as i32);
            e.visible = false;
            e.icon = true;
            self.add_effect(e);
        }
        // Consumption effects of the default components (golden apple, milk, honey…).
        let Some(c) = self.comp_json(s, "minecraft:consumable") else { return };
        self.run_consume_effects(c["on_consume_effects"].as_array().map_or(&[][..], Vec::as_slice));
    }

    /// Consumption effects applied to a mob that eats (fox, G5-F10): same kinds as for the player,
    /// draws from the mob's random, sound to its category.
    pub(crate) fn run_mob_consume_effects(&mut self, m: &mut crate::mobs::mob::Mob, list: &[serde_json::Value]) {
        for eff in list {
            match eff["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
                "apply_effects" => {
                    let p = eff["probability"].as_f64().unwrap_or(1.0) as f32;
                    if m.rng.next_float() >= p {
                        continue;
                    }
                    for e in eff["effects"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                        if let Some(i) = Inst::from_json(e) {
                            self.add_mob_effect(m, i);
                        }
                    }
                }
                "remove_effects" => {
                    let ids: Vec<&str> = match &eff["effects"] {
                        serde_json::Value::String(s) => vec![s.as_str()],
                        serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect(),
                        _ => Vec::new(),
                    };
                    for id in ids {
                        self.remove_mob_effect(m, id);
                    }
                }
                "clear_all_effects" => {
                    self.clear_mob_effects(m);
                }
                "teleport_randomly" => {
                    let trail = eff["directional_particles"].as_bool().unwrap_or(true);
                    self.chorus_teleport_mob(m, eff["diameter"].as_f64().unwrap_or(16.0), trail);
                }
                "play_sound" => {
                    if let Some(snd) = eff["sound"].as_str() {
                        let cat = Self::mob_sound_source(m);
                        self.play_sound(snd, cat, m.pos, 1.0, 1.0);
                    }
                }
                _ => {}
            }
        }
    }

    /// List of consumption effects (`ConsumeEffect`), shared with death protection
    /// (W8c).
    pub(crate) fn run_consume_effects(&mut self, list: &[serde_json::Value]) {
        for eff in list.iter().cloned() {
            match eff["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
                "apply_effects" => {
                    let p = eff["probability"].as_f64().unwrap_or(1.0) as f32;
                    if self.life.rng.next_float() >= p {
                        continue;
                    }
                    for e in eff["effects"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                        if let Some(i) = Inst::from_json(e) {
                            self.add_effect(i);
                        }
                    }
                }
                "remove_effects" => {
                    let ids: Vec<String> = match &eff["effects"] {
                        serde_json::Value::String(s) => vec![s.clone()],
                        serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                        _ => Vec::new(),
                    };
                    for id in ids {
                        self.remove_effect(&id);
                    }
                }
                "clear_all_effects" => {
                    self.clear_effects();
                }
                // S8: chorus fruit.
                "teleport_randomly" => {
                    let trail = eff["directional_particles"].as_bool().unwrap_or(true);
                    self.chorus_teleport(eff["diameter"].as_f64().unwrap_or(16.0), trail);
                }
                "play_sound" => {
                    if let Some(snd) = eff["sound"].as_str() {
                        self.play_sound(snd, 7, self.pos, 1.0, 1.0);
                    }
                }
                _ => {}
            }
        }
    }
}
