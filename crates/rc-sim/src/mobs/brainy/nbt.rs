//! Saved brain memories: those that have a codec and that the species' brain registers; value,
//! and remaining duration if it expires.

use rc_proto::nbt::Nbt;

use super::super::species::Kind;
use super::super::villager::brain::{DimKey, GPos};
use super::brain::{Mem, NEVER, Slot};
use crate::nbtio;

fn entry(v: Nbt, ttl: i64) -> Nbt {
    let mut f = vec![("value".to_owned(), v)];
    if ttl != NEVER {
        f.push(("ttl".to_owned(), Nbt::Long(ttl)));
    }
    Nbt::Compound(f)
}

fn unit() -> Nbt {
    Nbt::Compound(Vec::new())
}

fn gpos(g: GPos) -> Nbt {
    Nbt::Compound(vec![("pos".into(), Nbt::IntArray(g.pos.to_vec())), ("dimension".into(), Nbt::String(g.dim.name()))])
}

fn read_gpos(v: &Nbt) -> Option<GPos> {
    let pos = match v.get("pos")? {
        Nbt::IntArray(a) if a.len() == 3 => [a[0], a[1], a[2]],
        _ => return None,
    };
    let Nbt::String(d) = v.get("dimension")? else { return None };
    let dim = rc_data::ids::with_full_id(d, DimKey::of);
    Some(GPos { dim, pos })
}

/// Memories registered by species (names without namespace).
fn names(k: Kind) -> &'static [&'static str] {
    match k {
        Kind::Goat => &["long_jump_cooling_down", "ram_cooldown_ticks", "temptation_cooldown_ticks", "is_panicking"],
        Kind::Frog => {
            &["long_jump_cooling_down", "temptation_cooldown_ticks", "is_in_water", "is_pregnant", "is_panicking"]
        }
        Kind::Tadpole => &["temptation_cooldown_ticks", "is_panicking"],
        Kind::Armadillo => {
            &["danger_detected_recently", "temptation_cooldown_ticks", "gaze_cooldown_ticks", "is_panicking"]
        }
        Kind::Sniffer => &["sniffer_explored_positions", "sniff_cooldown", "temptation_cooldown_ticks", "is_panicking"],
        Kind::Axolotl => &["play_dead_ticks", "has_hunting_cooldown", "temptation_cooldown_ticks"],
        // F14: serializable memories of the warden, breeze and happy ghast brains.
        Kind::Warden => &[
            "recent_projectile",
            "is_sniffing",
            "is_emerging",
            "roar_sound_delay",
            "dig_cooldown",
            "roar_sound_cooldown",
            "sniff_cooldown",
            "touch_cooldown",
            "vibration_cooldown",
            "sonic_boom_cooldown",
            "sonic_boom_sound_cooldown",
            "sonic_boom_sound_delay",
        ],
        Kind::Breeze => &[
            "breeze_jump_cooldown",
            "breeze_shoot",
            "breeze_shoot_charging",
            "breeze_shoot_recover",
            "breeze_shoot_cooldown",
            "breeze_jump_inhaling",
            "breeze_jump_target",
            "breeze_leaving_water",
        ],
        Kind::HappyGhast => &["temptation_cooldown_ticks", "is_panicking"],
        Kind::Creaking => &[],
        _ => &[
            "liked_player",
            "liked_noteblock",
            "liked_noteblock_cooldown_ticks",
            "item_pickup_cooldown_ticks",
            "is_panicking",
        ],
    }
}

fn int_slot<'a>(m: &'a Mem, n: &str) -> Option<&'a Slot<i32>> {
    Some(match n {
        "long_jump_cooling_down" => &m.lj_cd,
        "ram_cooldown_ticks" => &m.ram_cd,
        "temptation_cooldown_ticks" => &m.tempt_cd,
        "gaze_cooldown_ticks" => &m.gaze_cd,
        "play_dead_ticks" => &m.play_dead,
        "liked_noteblock_cooldown_ticks" => &m.liked_noteblock_cd,
        "item_pickup_cooldown_ticks" => &m.pickup_cd,
        _ => return None,
    })
}

fn int_slot_mut<'a>(m: &'a mut Mem, n: &str) -> Option<&'a mut Slot<i32>> {
    Some(match n {
        "long_jump_cooling_down" => &mut m.lj_cd,
        "ram_cooldown_ticks" => &mut m.ram_cd,
        "temptation_cooldown_ticks" => &mut m.tempt_cd,
        "gaze_cooldown_ticks" => &mut m.gaze_cd,
        "play_dead_ticks" => &mut m.play_dead,
        "liked_noteblock_cooldown_ticks" => &mut m.liked_noteblock_cd,
        "item_pickup_cooldown_ticks" => &mut m.pickup_cd,
        _ => return None,
    })
}

fn bool_slot_mut<'a>(m: &'a mut Mem, n: &str) -> Option<&'a mut Slot<bool>> {
    Some(match n {
        "is_panicking" => &mut m.panicking,
        "danger_detected_recently" => &mut m.danger,
        "has_hunting_cooldown" => &mut m.hunting_cd,
        _ => return None,
    })
}

fn bool_slot<'a>(m: &'a Mem, n: &str) -> Option<&'a Slot<bool>> {
    Some(match n {
        "is_panicking" => &m.panicking,
        "danger_detected_recently" => &m.danger,
        "has_hunting_cooldown" => &m.hunting_cd,
        _ => return None,
    })
}

fn unit_slot<'a>(m: &'a Mem, n: &str) -> Option<&'a Slot<()>> {
    Some(match n {
        "recent_projectile" => &m.x.recent_projectile,
        "is_sniffing" => &m.x.is_sniffing,
        "is_emerging" => &m.x.is_emerging,
        "roar_sound_delay" => &m.x.roar_sound_delay,
        "dig_cooldown" => &m.x.dig_cooldown,
        "roar_sound_cooldown" => &m.x.roar_sound_cooldown,
        "touch_cooldown" => &m.x.touch_cooldown,
        "vibration_cooldown" => &m.x.vibration_cooldown,
        "sonic_boom_cooldown" => &m.x.sonic_cooldown,
        "sonic_boom_sound_cooldown" => &m.x.sonic_sound_cooldown,
        "sonic_boom_sound_delay" => &m.x.sonic_sound_delay,
        "breeze_jump_cooldown" => &m.x.jump_cooldown,
        "breeze_shoot" => &m.x.shoot,
        "breeze_shoot_charging" => &m.x.shoot_charging,
        "breeze_shoot_recover" => &m.x.shoot_recovering,
        "breeze_shoot_cooldown" => &m.x.shoot_cooldown,
        "breeze_jump_inhaling" => &m.x.jump_inhaling,
        "breeze_leaving_water" => &m.x.leaving_water,
        "is_in_water" => &m.in_water,
        "is_pregnant" => &m.pregnant,
        "sniff_cooldown" => &m.sniff_cd,
        _ => return None,
    })
}

fn unit_slot_mut<'a>(m: &'a mut Mem, n: &str) -> Option<&'a mut Slot<()>> {
    Some(match n {
        "recent_projectile" => &mut m.x.recent_projectile,
        "is_sniffing" => &mut m.x.is_sniffing,
        "is_emerging" => &mut m.x.is_emerging,
        "roar_sound_delay" => &mut m.x.roar_sound_delay,
        "dig_cooldown" => &mut m.x.dig_cooldown,
        "roar_sound_cooldown" => &mut m.x.roar_sound_cooldown,
        "touch_cooldown" => &mut m.x.touch_cooldown,
        "vibration_cooldown" => &mut m.x.vibration_cooldown,
        "sonic_boom_cooldown" => &mut m.x.sonic_cooldown,
        "sonic_boom_sound_cooldown" => &mut m.x.sonic_sound_cooldown,
        "sonic_boom_sound_delay" => &mut m.x.sonic_sound_delay,
        "breeze_jump_cooldown" => &mut m.x.jump_cooldown,
        "breeze_shoot" => &mut m.x.shoot,
        "breeze_shoot_charging" => &mut m.x.shoot_charging,
        "breeze_shoot_recover" => &mut m.x.shoot_recovering,
        "breeze_shoot_cooldown" => &mut m.x.shoot_cooldown,
        "breeze_jump_inhaling" => &mut m.x.jump_inhaling,
        "breeze_leaving_water" => &mut m.x.leaving_water,
        "is_in_water" => &mut m.in_water,
        "is_pregnant" => &mut m.pregnant,
        "sniff_cooldown" => &mut m.sniff_cd,
        _ => return None,
    })
}

/// `Brain` compound to write.
pub fn save_memories(k: Kind, m: &Mem) -> Nbt {
    let mut f: Vec<(String, Nbt)> = Vec::new();
    for &n in names(k) {
        let key = format!("minecraft:{n}");
        let v = if let Some(s) = int_slot(m, n) {
            s.v.map(|v| entry(Nbt::Int(v), s.ttl))
        } else if let Some(s) = bool_slot(m, n) {
            s.v.map(|v| entry(Nbt::Byte(i8::from(v)), s.ttl))
        } else if let Some(s) = unit_slot(m, n) {
            s.v.map(|_| entry(unit(), s.ttl))
        } else {
            match n {
                "liked_player" => m.liked_player.v.map(|u| entry(nbtio::uuid_nbt(u), m.liked_player.ttl)),
                "liked_noteblock" => m.liked_noteblock.v.map(|g| entry(gpos(g), m.liked_noteblock.ttl)),
                "breeze_jump_target" => m.x.jump_target.v.map(|b| entry(Nbt::IntArray(b.to_vec()), m.x.jump_target.ttl)),
                "sniffer_explored_positions" => m
                    .explored
                    .v
                    .as_ref()
                    .map(|l| entry(Nbt::List(l.iter().map(|g| gpos(*g)).collect()), m.explored.ttl)),
                _ => None,
            }
        };
        if let Some(v) = v {
            f.push((key, v));
        }
    }
    Nbt::Compound(vec![("memories".into(), Nbt::Compound(f))])
}

/// Memories read back.
pub fn load_memories(m: &mut Mem, brain: Option<&Nbt>) {
    let Some(Nbt::Compound(f)) = brain.and_then(|b| b.get("memories")) else { return };
    for (k, v) in f {
        let n = k.trim_start_matches("minecraft:");
        let ttl = v.get("ttl").and_then(Nbt::as_i64).unwrap_or(NEVER);
        let Some(val) = v.get("value") else { continue };
        if let Some(s) = int_slot_mut(m, n) {
            if let Some(i) = val.as_i64() {
                s.set_exp(i as i32, ttl);
            }
        } else if let Some(s) = bool_slot_mut(m, n) {
            if let Some(i) = val.as_i64() {
                s.set_exp(i != 0, ttl);
            }
        } else if let Some(s) = unit_slot_mut(m, n) {
            s.set_exp((), ttl);
        } else {
            match n {
                "liked_player" => {
                    if let Some(u) = nbtio::uuid(Some(val)) {
                        m.liked_player.set_exp(u, ttl);
                    }
                }
                "liked_noteblock" => {
                    if let Some(g) = read_gpos(val) {
                        m.liked_noteblock.set_exp(g, ttl);
                    }
                }
                "breeze_jump_target" => {
                    if let Nbt::IntArray(v) = val
                        && v.len() == 3
                    {
                        m.x.jump_target.set_exp([v[0], v[1], v[2]], ttl);
                    }
                }
                "sniffer_explored_positions" => {
                    if let Nbt::List(l) = val {
                        let list: Vec<GPos> = l.iter().filter_map(read_gpos).collect();
                        if !list.is_empty() {
                            m.explored.set_exp(list, ttl);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
