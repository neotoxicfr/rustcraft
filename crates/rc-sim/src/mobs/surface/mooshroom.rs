//! Mooshroom (§ 4.3): the cow's goals, stew (suspicious), shearing into a cow, flowers, lightning.

use rc_proto::nbt::Nbt;

use super::super::goals::Goal;
use super::super::mob::Mob;
use super::super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

pub fn register(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(0, Goal::Float);
    g.add(1, super::panic(2.0, "minecraft:panic_causes"));
    g.add(2, super::breed_goal(1.0));
    g.add(3, super::tempt(1.25, "minecraft:cow_food", false));
    g.add(4, super::follow_parent(1.25));
    g.add(5, super::stroll(1.0));
    g.add(6, super::look_player(6.0));
    g.add(7, super::look_around());
}

/// Stew effect of a flower (effect, duration in ticks = ⌊seconds × 20⌋).
pub fn flower_effect(item: &str) -> Option<(&'static str, i32)> {
    let (e, secs): (&str, f32) = match item.trim_start_matches("minecraft:") {
        "dandelion" | "golden_dandelion" | "blue_orchid" => ("saturation", 0.35),
        "torchflower" | "poppy" => ("night_vision", 5.0),
        "allium" => ("fire_resistance", 3.0),
        "azure_bluet" | "open_eyeblossom" => ("blindness", 11.0),
        "red_tulip" | "orange_tulip" | "white_tulip" | "pink_tulip" => ("weakness", 7.0),
        "oxeye_daisy" => ("regeneration", 7.0),
        "cornflower" => ("jump_boost", 5.0),
        "wither_rose" => ("wither", 7.0),
        "lily_of_the_valley" => ("poison", 11.0),
        "closed_eyeblossom" => ("nausea", 7.0),
        _ => return None,
    };
    let id = match e {
        "saturation" => "minecraft:saturation",
        "night_vision" => "minecraft:night_vision",
        "fire_resistance" => "minecraft:fire_resistance",
        "blindness" => "minecraft:blindness",
        "weakness" => "minecraft:weakness",
        "regeneration" => "minecraft:regeneration",
        "jump_boost" => "minecraft:jump_boost",
        "wither" => "minecraft:wither",
        "poison" => "minecraft:poison",
        _ => "minecraft:nausea",
    };
    Some((id, (secs * 20.0).floor() as i32))
}

/// Effect list (the game's NBT format: `id`, `duration`).
fn effect_list(id: &str, duration: i32) -> Nbt {
    Nbt::List(vec![Nbt::Compound(vec![("id".into(), Nbt::String(id.into())), ("duration".into(), Nbt::Int(duration))])])
}

/// Empties the held item, stores the full one (or drops it).
pub(crate) fn fill_held(s: &mut Server, slot: usize, result: Stack, limit_creative: bool) {
    let creative = s.game_mode == 1;
    let held = s.held as usize;
    if limit_creative && creative {
        let has = s.slots.iter().flatten().any(|t| t.id == result.id && t.components == result.components);
        if !has {
            let _ = s.pickup_into_inventory(result, true, held);
        }
        return;
    }
    if !creative && let Some(st) = &mut s.slots[slot] {
        st.count -= 1;
        if st.count <= 0 {
            s.slots[slot] = None;
        }
    }
    if s.slots[slot].is_none() {
        s.slots[slot] = Some(result);
        return;
    }
    if let Some(rest) = s.pickup_into_inventory(result, creative, held) {
        s.drop_from_player(rest, false);
    }
}

pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item == "minecraft:bowl" && !m.is_baby() {
        let suspicious = m.surf.stew.is_some();
        let stew = match m.surf.stew.take() {
            Some(eff) => {
                let mut st = Stack { id: "minecraft:suspicious_stew".into(), count: 1, components: None };
                crate::items::set_component(&mut st, "minecraft:suspicious_stew_effects", eff);
                st
            }
            None => Stack { id: "minecraft:mushroom_stew".into(), count: 1, components: None },
        };
        fill_held(s, slot, stew, false);
        let snd =
            if suspicious { "minecraft:entity.mooshroom.suspicious_milk" } else { "minecraft:entity.mooshroom.milk" };
        s.mob_sound(m, snd, 1.0, 1.0);
        return Some(true);
    }
    if item == "minecraft:shears" && !m.is_baby() {
        shear(s, m);
        s.sheared_by_player(m);
        s.hurt_item(slot, 1);
        return Some(true);
    }
    if m.variant == 1 && !m.is_baby() {
        let Some((eff, dur)) = flower_effect(item) else {
            return bucket(s, m, slot, item);
        };
        let center = [m.pos[0], m.pos[1] + f64::from(m.height()) * 0.5, m.pos[2]];
        if m.surf.stew.is_some() {
            s.particles_full("minecraft:smoke", 0, center, [0.5, 0.0, 0.5], [0.0, 0.2, 0.0], 2, 2);
        } else {
            if s.game_mode != 1
                && let Some(st) = &mut s.slots[slot]
            {
                st.count -= 1;
                if st.count <= 0 {
                    s.slots[slot] = None;
                }
            }
            s.particles_full("minecraft:effect", 0xFFFF_FFFF, center, [0.5, 0.0, 0.5], [0.0, 0.2, 0.0], 4, 2);
            m.surf.stew = Some(effect_list(eff, dur));
            s.mob_sound(m, "minecraft:entity.mooshroom.eat", 2.0, 1.0);
        }
        return Some(true);
    }
    bucket(s, m, slot, item)
}

/// Bucket → milk (otherwise animal food).
fn bucket(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item == "minecraft:bucket" && !m.is_baby() {
        s.world_sound("minecraft:entity.cow.milk", 7, s.pos, 1.0, 1.0);
        let milk = Stack { id: "minecraft:milk_bucket".into(), count: 1, components: None };
        fill_held(s, slot, milk, true);
        return Some(true);
    }
    None
}

/// Shearing: sound, conversion into a cow (same place, same flags), explosion, mushrooms.
fn shear(s: &mut Server, m: &mut Mob) {
    s.entity_sound(m.id, "minecraft:entity.mooshroom.shear", 7, 1.0, 1.0);
    // No spawn preparation: default variant ("temperate", set at
    // construction).
    let mut cow = s.new_mob(Kind::Cow, m.pos, m.yaw);
    cow.pitch = m.pitch;
    cow.vel = m.vel;
    cow.fall_distance = m.fall_distance;
    cow.last_hurt_by_player = m.last_hurt_by_player;
    cow.hurt_time = m.hurt_time;
    cow.body_yaw = m.body_yaw;
    cow.on_ground = m.on_ground;
    cow.age = m.age;
    cow.left_handed = m.left_handed;
    cow.no_ai = m.no_ai;
    cow.persistent = m.persistent;
    cow.fire = m.fire;
    cow.surf.no_gravity = m.surf.no_gravity;
    cow.custom_name = m.custom_name.clone();
    let center = [m.pos[0], m.pos[1] + f64::from(m.height()) * 0.5, m.pos[2]];
    s.particles("minecraft:explosion", center, [0.0; 3], 0.0, 1);
    let variant = if m.variant == 1 { "brown" } else { "red" };
    let ctx =
        crate::loot::Ctx { comps: vec![("minecraft:mooshroom/variant".into(), variant.into())], ..Default::default() };
    let drops = super::loot(s, "minecraft:shearing/mooshroom", &ctx);
    let top = [m.pos[0], m.pos[1] + f64::from(m.height()), m.pos[2]];
    for d in drops {
        for _ in 0..d.count {
            // Creation of the item entity: velocity drawn on the world random.
            let r = &mut s.mobs.rng;
            let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
            s.spawn_item(top, v, Stack { count: 1, ..d.clone() }, 0);
        }
    }
    s.add_mob(cow);
    m.removed = true;
}

/// Lightning: toggles red ↔ brown once per lightning.
pub fn thunder_hit(s: &mut Server, m: &mut Mob, bolt: u128) {
    if m.surf.last_bolt != Some(bolt) {
        m.variant = 1 - m.variant;
        m.surf.last_bolt = Some(bolt);
        s.mob_sound(m, "minecraft:entity.mooshroom.convert", 2.0, 1.0);
    }
}
