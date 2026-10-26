//! Singleplayer player attributes: the attribute table (default, bounds, synchronisation), tracking
//! of the modifiers of worn equipment (the `attribute_modifiers` component and the attribute
//! effects of enchantments) tick after tick, the full clamped value, and sending the client only
//! the attributes that changed (`update_attributes`).
//!
//! Equipment tracking: on every player tick, each slot (main hand, off hand, feet, legs, chest,
//! head, in that order) is compared with the stack seen on the previous tick; for each changed
//! slot, the modifiers of the old stack are removed (by identifier), then those of the new stack
//! (if it is not broken) are applied, replacing the identifier. As in the game, two slots that
//! apply the same identifier get in each other's way: removing one also removes the other until
//! the latter changes again.

use hashbrown::HashMap;

use rc_proto::nbt::Nbt;

use crate::player::Stack;
use crate::server::Server;

/// Attribute: identifier, default, minimum, maximum, sent to the client.
pub(crate) struct AttrDef {
    pub name: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub synced: bool,
}

const fn a(name: &'static str, default: f64, min: f64, max: f64, synced: bool) -> AttrDef {
    AttrDef { name, default, min, max, synced }
}

/// Attributes of game 26.3 (bounds and synchronisation from the registry).
pub(crate) const ATTRS: &[AttrDef] = &[
    a("minecraft:air_drag_modifier", 1.0, 0.0, 2048.0, true),
    a("minecraft:armor", 0.0, 0.0, 30.0, true),
    a("minecraft:armor_toughness", 0.0, 0.0, 20.0, true),
    a("minecraft:attack_damage", 2.0, 0.0, 2048.0, false),
    a("minecraft:attack_knockback", 0.0, 0.0, 5.0, false),
    a("minecraft:attack_speed", 4.0, 0.0, 1024.0, true),
    a("minecraft:below_name_distance", 10.0, 0.0, 512.0, true),
    a("minecraft:block_break_speed", 1.0, 0.0, 1024.0, true),
    a("minecraft:block_interaction_range", 4.5, 0.0, 64.0, true),
    a("minecraft:bounciness", 0.0, 0.0, 1.0, true),
    a("minecraft:burning_time", 1.0, 0.0, 1024.0, true),
    a("minecraft:camera_distance", 4.0, 0.0, 32.0, true),
    a("minecraft:explosion_knockback_resistance", 0.0, 0.0, 1.0, true),
    a("minecraft:entity_interaction_range", 3.0, 0.0, 64.0, true),
    a("minecraft:fall_damage_multiplier", 1.0, 0.0, 100.0, true),
    a("minecraft:flying_speed", 0.4, 0.0, 1024.0, true),
    a("minecraft:follow_range", 32.0, 0.0, 2048.0, false),
    a("minecraft:friction_modifier", 1.0, 0.0, 2048.0, true),
    a("minecraft:gravity", 0.08, -1.0, 1.0, true),
    a("minecraft:jump_strength", 0.42f32 as f64, 0.0, 32.0, true),
    a("minecraft:knockback_resistance", 0.0, -2.0, 1.0, false),
    a("minecraft:luck", 0.0, -1024.0, 1024.0, true),
    a("minecraft:max_absorption", 0.0, 0.0, 2048.0, true),
    a("minecraft:max_health", 20.0, 1.0, 1024.0, true),
    a("minecraft:mining_efficiency", 0.0, 0.0, 1024.0, true),
    a("minecraft:movement_efficiency", 0.0, 0.0, 1.0, true),
    a("minecraft:movement_speed", 0.7, 0.0, 1024.0, true),
    a("minecraft:name_tag_distance", 64.0, 0.0, 512.0, true),
    a("minecraft:oxygen_bonus", 0.0, 0.0, 1024.0, true),
    a("minecraft:safe_fall_distance", 3.0, -1024.0, 1024.0, true),
    a("minecraft:scale", 1.0, 0.0625, 16.0, true),
    a("minecraft:sneaking_speed", 0.3, 0.0, 1.0, true),
    a("minecraft:spawn_reinforcements", 0.0, 0.0, 1.0, false),
    a("minecraft:step_height", 0.6, 0.0, 10.0, true),
    a("minecraft:submerged_mining_speed", 0.2, 0.0, 20.0, true),
    a("minecraft:sweeping_damage_ratio", 0.0, 0.0, 1.0, true),
    a("minecraft:tempt_range", 10.0, 0.0, 2048.0, false),
    a("minecraft:water_movement_efficiency", 0.0, 0.0, 1.0, true),
    a("minecraft:waypoint_transmit_range", 0.0, 0.0, 6.0e7, false),
    a("minecraft:waypoint_receive_range", 0.0, 0.0, 6.0e7, false),
];

pub(crate) fn def(name: &str) -> Option<&'static AttrDef> {
    ATTRS.iter().find(|d| d.name == name)
}

/// Clamps an attribute (`NaN`: minimum), as the game does on every value computation.
pub(crate) fn sanitize(name: &str, v: f64) -> f64 {
    match def(name) {
        Some(d) if v.is_nan() => d.min,
        Some(d) => v.clamp(d.min, d.max),
        None => v,
    }
}

/// Player equipment slots in the game's order and their group name.
pub(crate) const SLOTS: [&str; 6] = ["mainhand", "offhand", "feet", "legs", "chest", "head"];

/// Does the slot group `group` (the `slot` field of a modifier, the `slots` of an
/// enchantment) contain the slot `slot`?
pub(crate) fn group_has(group: &str, slot: &str) -> bool {
    match group {
        "any" => true,
        "hand" => matches!(slot, "mainhand" | "offhand"),
        "armor" => matches!(slot, "feet" | "legs" | "chest" | "head" | "body"),
        g => g == slot,
    }
}

fn op_of(s: Option<&str>) -> i32 {
    match s {
        Some("add_multiplied_base") => 1,
        Some("add_multiplied_total") => 2,
        _ => 0,
    }
}

/// Applied equipment modifier: attribute, identifier, value, operation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EquipMod {
    pub attr: String,
    pub id: String,
    pub amount: f64,
    pub op: i32,
}

/// Sent attribute: base and modifiers (identifier, value, operation).
type Sent = (f64, Vec<(String, f64, i32)>);

/// Tracked equipment state and the attributes already sent.
#[derive(Default)]
pub struct EquipAttrs {
    /// Stack seen on the previous tick, per slot (`SLOTS`).
    last: [Option<Stack>; 6],
    /// Modifiers applied by the equipment (one per attribute/identifier pair).
    pub(crate) mods: Vec<EquipMod>,
    /// Attributes sent to the client: base and modifiers.
    sent: HashMap<i32, Sent>,
}

impl EquipAttrs {
    /// The client recreated its player (respawn): everything must be sent again.
    pub fn forget_sent(&mut self) {
        self.sent.clear();
    }

    fn remove(&mut self, attr: &str, id: &str) {
        self.mods.retain(|m| !(m.attr == attr && m.id == id));
    }
}

impl Server {
    /// Stack worn in an equipment slot (`SLOTS`).
    pub(crate) fn equip_stack(&self, k: usize) -> Option<&Stack> {
        let i = match k {
            0 => 36 + self.held as usize,
            1 => 45,
            2 => 8,
            3 => 7,
            4 => 6,
            _ => 5,
        };
        self.slots.get(i).and_then(Option::as_ref)
    }

    /// Modifiers of a stack for a slot: the `attribute_modifiers` component (the stack's or the
    /// item's default), then the attribute effects of enchantments whose slots
    /// cover this one (identifier suffixed with the slot name).
    pub(crate) fn stack_mods(&self, s: &Stack, slot: &str) -> Vec<EquipMod> {
        let mut out = Vec::new();
        let patched = crate::items::component(s, "minecraft:attribute_modifiers");
        if let Some(n) = patched {
            let list = match n {
                Nbt::List(l) => l.as_slice(),
                o => o
                    .get("modifiers")
                    .and_then(|m| if let Nbt::List(l) = m { Some(l.as_slice()) } else { None })
                    .unwrap_or(&[]),
            };
            for m in list {
                let group = m.get("slot").and_then(Nbt::as_str).unwrap_or("any");
                if !group_has(group, slot) {
                    continue;
                }
                let (Some(attr), Some(id)) = (m.get("type").and_then(Nbt::as_str), m.get("id").and_then(Nbt::as_str))
                else {
                    continue;
                };
                let amount = m.get("amount").and_then(crate::mobs::mob::f64_of).unwrap_or(0.0);
                out.push(EquipMod {
                    attr: attr.to_owned(),
                    id: id.to_owned(),
                    amount,
                    op: op_of(m.get("operation").and_then(Nbt::as_str)),
                });
            }
        } else if let Some(v) = self.life.items.default_component(s, "minecraft:attribute_modifiers") {
            let list = v.as_array().or_else(|| v["modifiers"].as_array()).map(Vec::as_slice).unwrap_or(&[]);
            for m in list {
                if !group_has(m["slot"].as_str().unwrap_or("any"), slot) {
                    continue;
                }
                let (Some(attr), Some(id)) = (m["type"].as_str(), m["id"].as_str()) else { continue };
                out.push(EquipMod {
                    attr: attr.to_owned(),
                    id: id.to_owned(),
                    amount: m["amount"].as_f64().unwrap_or(0.0),
                    op: op_of(m["operation"].as_str()),
                });
            }
        }
        for (ench, lvl) in crate::enchanting::read_comp(&self.life.items, s, "minecraft:enchantments") {
            let Some(d) = self.life.pack.get("enchantment", &ench) else { continue };
            let slots = d["slots"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            if !slots.iter().any(|g| g.as_str().is_some_and(|g| group_has(g, slot))) {
                continue;
            }
            let effects = d["effects"]["minecraft:attributes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            for e in effects {
                let (Some(attr), Some(id)) = (e["attribute"].as_str(), e["id"].as_str()) else { continue };
                let amount = crate::projectiles::ench::level_value(&e["amount"], lvl);
                out.push(EquipMod {
                    attr: attr.to_owned(),
                    id: format!("{}/{slot}", full_id(id)),
                    amount: f64::from(amount),
                    op: op_of(e["operation"].as_str()),
                });
            }
        }
        out
    }

    /// Broken stack: usable and worn down to the end (its modifiers do not apply).
    fn stack_broken(&self, s: &Stack) -> bool {
        let items = &self.life.items;
        items.damageable(s) && items.damage(s) >= items.max_damage(s)
    }

    /// Player tick: equipment changes spotted, modifiers updated.
    pub(crate) fn equipment_tick(&mut self) {
        let mut changed: Vec<usize> = Vec::new();
        for (k, slot) in SLOTS.iter().enumerate() {
            let cur = self.equip_stack(k).cloned();
            let prev = self.life.attrs.last[k].clone();
            if crate::items::matches(prev.as_ref(), cur.as_ref()) {
                continue;
            }
            changed.push(k);
            if let Some(p) = &prev {
                for m in self.stack_mods(p, slot) {
                    self.life.attrs.remove(&m.attr, &m.id);
                }
            }
            self.life.attrs.last[k] = cur;
        }
        for k in changed {
            let Some(cur) = self.life.attrs.last[k].clone() else { continue };
            if self.stack_broken(&cur) {
                continue;
            }
            for m in self.stack_mods(&cur, SLOTS[k]) {
                self.life.attrs.remove(&m.attr, &m.id);
                self.life.attrs.mods.push(m);
            }
        }
    }

    /// Modifiers of the worn equipment for an attribute.
    pub(crate) fn equip_mods(&self, name: &str) -> Vec<(String, f64, i32)> {
        self.life.attrs.mods.iter().filter(|m| m.attr == name).map(|m| (m.id.clone(), m.amount, m.op)).collect()
    }

    /// Creative mode modifiers (ranges).
    pub(crate) fn creative_mods(&self, name: &str) -> Vec<(String, f64, i32)> {
        if self.game_mode != 1 {
            return Vec::new();
        }
        match name {
            "minecraft:block_interaction_range" => vec![("minecraft:creative_mode_block_range".into(), 0.5, 0)],
            "minecraft:entity_interaction_range" => vec![("minecraft:creative_mode_entity_range".into(), 2.0, 0)],
            _ => Vec::new(),
        }
    }

    /// Full value of a player attribute (base, `/attribute`, effects, mode, equipment),
    /// clamped as the game does.
    pub(crate) fn player_attribute(&self, name: &str) -> f64 {
        let v = crate::effects::attribute_value(self.base_attr(name), &self.player_mods(name));
        sanitize(name, v)
    }

    /// Sends the transmissible attributes that changed since the last send (base or
    /// modifiers); an attribute that went back to its default state is sent without modifiers.
    pub(crate) fn sync_attributes(&mut self) {
        use rc_proto_v777::play::{Attribute, Modifier};
        let Some(reg) = self.data.registry("minecraft:attribute") else { return };
        let mut out: Vec<Attribute> = Vec::new();
        let mut now: HashMap<i32, Sent> = HashMap::new();
        for d in ATTRS.iter().filter(|d| d.synced) {
            let Some(id) = reg.entries.iter().position(|e| e == d.name).map(|i| i as i32) else { continue };
            let base = self.base_attr(d.name);
            let mods = self.player_mods(d.name);
            let default_base = crate::effects::player_base_or(d.name, d.default);
            if mods.is_empty() && base == default_base && !self.life.attrs.sent.contains_key(&id) {
                continue;
            }
            now.insert(id, (base, mods));
        }
        let mut ids: Vec<i32> = now.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let cur = &now[&id];
            if self.life.attrs.sent.get(&id) == Some(cur) {
                continue;
            }
            let modifiers = cur
                .1
                .iter()
                .map(|(mid, amount, op)| Modifier { id: mid.clone(), amount: *amount, operation: *op })
                .collect();
            out.push(Attribute { id, base: cur.0, modifiers });
            self.life.attrs.sent.insert(id, cur.clone());
        }
        if !out.is_empty() {
            let _ = self.send(rc_client::Event::Attributes { entity: self.player_id, attributes: out });
        }
    }
}

/// Full identifier (`minecraft:` implicit).
fn full_id(id: &str) -> String {
    if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") }
}
