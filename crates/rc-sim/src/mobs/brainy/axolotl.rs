//! Axolotl: brain (core, idle, fight, play dead), hunting in water (hostiles always, prey outside
//! the 2,400-tick cooldown), playing dead (regeneration), player support (regeneration, mining
//! fatigue removed), 6,000 air (rain included), variants, bucket.

use rc_proto::nbt::Nbt;

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::brain::*;
use crate::nbtio;
use crate::player::Stack;
use crate::server::Server;

pub const MAX_AIR: i32 = 6000;

/// Variants: lucy, wild, gold, cyan (common), blue (rare).
pub const VARIANTS: [&str; 5] = ["lucy", "wild", "gold", "cyan", "blue"];

pub fn brain(rng: &mut JRand) -> Brain {
    use K::*;
    use St::*;
    let core_nodes = seq(
        0,
        Act::Core,
        vec![Bk::LookSink { lo: 45, hi: 90 }, move_sink(), Bk::ValidatePlayDead, Bk::CountDown(TemptCd)],
    );
    let idle = vec![
        at(0, Act::Idle, look_sometimes(6.0, true)),
        at(1, Act::Idle, Bk::MakeLove { speed: 0.2, close: 2, at: 0, sniffer: false }),
        at(
            2,
            Act::Idle,
            run_one(
                &[],
                0x4158_4f4c,
                vec![
                    (Bk::Tempt { speed: Spd::Water(0.5, 0.15), close: Close::Fixed(2.5), sniffer: false }, 1),
                    (Bk::BabyFollow { lo: 5, hi: 16, speed: Spd::Water(0.6, 0.15) }, 1),
                ],
            ),
        ),
        at(3, Act::Idle, Bk::StartAttacking(Finder::Attackable)),
        at(3, Act::Idle, Bk::FindLiquid { range: 6, speed: 0.15, next: 0 }),
        at(
            4,
            Act::Idle,
            try_all(
                &[(Walk, Absent)],
                vec![
                    (Bk::StrollSwim { speed: 0.5 }, 2),
                    (Bk::Stroll { speed: 0.15, from_water: false }, 2),
                    (Bk::WalkFromLook { speed: Spd::Water(0.5, 0.15), close: 3, axolotl: true }, 3),
                    (Bk::Trigger(Cond::InWater), 5),
                    (Bk::Trigger(Cond::OnGround), 5),
                ],
            ),
        ),
    ];
    let fight = seq(
        0,
        Act::Fight,
        vec![
            Bk::StopAttacking { axolotl: true },
            Bk::WalkToAttack(Spd::Water(0.6, 0.15)),
            Bk::Melee { cd: 20 },
            Bk::EraseIfBreeding(Attack),
        ],
    );
    let dead = vec![at(0, Act::PlayDead, Bk::PlayDead), at(1, Act::PlayDead, Bk::EraseIfBreeding(PlayDead))];
    Brain::new(
        rng,
        &[Sn::Living, Sn::Adult, Sn::HurtBy, Sn::AxolotlAttackables, Sn::FoodTempt],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Fight, &[(Attack, Present)], &[Attack]), fight),
            (act(Act::PlayDead, &[(PlayDead, Present)], &[PlayDead]), dead),
        ],
    )
}

/// Never left "play dead" through this choice; 2,400-tick hunt
/// cooldown on leaving combat.
pub fn update_activity(b: &mut Brain) {
    let old = b.act;
    if old != Act::PlayDead {
        b.first_valid(&[Act::PlayDead, Act::Fight, Act::Idle]);
        if old == Act::Fight && b.act != Act::Fight {
            b.mem.hunting_cd.set_exp(true, 2400);
        }
    }
}

/// Finalization: two common variants drawn for the group, one of the two for each member; from
/// the third member on, a baby.
pub fn finalize(s: &mut Server, m: &mut Mob, group: &mut super::super::spawn::Group) {
    let baby = match group.axolotl {
        Some(_) => group.size >= 2,
        None => {
            let a = s.mobs.rng.next_int_bound(4);
            let b = s.mobs.rng.next_int_bound(4);
            group.axolotl = Some([a, b]);
            false
        }
    };
    group.no_baby = true;
    let types = group.axolotl.unwrap_or([0, 0]);
    m.variant = types[s.mobs.rng.next_int_bound(2) as usize];
    if baby {
        m.age = -24000;
    }
}

/// Baby: 1 in 1,200 the rare variant, otherwise a parent's; persistent.
pub fn offspring(s: &mut Server, m: &mut Mob, partner: i32) -> Mob {
    let mut baby = s.new_mob(super::super::species::Kind::Axolotl, m.pos, 0.0);
    baby.variant = if m.rng.next_int_bound(1200) == 0 {
        let _ = m.rng.next_int_bound(1);
        4
    } else if m.rng.next_bool() {
        m.variant
    } else {
        s.mob_ref(partner).map_or(m.variant, |p| p.variant)
    };
    baby.persistent = true;
    baby
}

/// Play dead on hit (1 in 3, then 1 in 3 under the damage or below half health), non-lethal
/// damage, in water, hit coming from an entity.
pub fn maybe_play_dead(_s: &mut Server, m: &mut Mob, damage: f32, by: Option<Ent>) {
    if m.no_ai {
        return;
    }
    if m.rng.next_int_bound(3) != 0 {
        return;
    }
    let r = m.rng.next_int_bound(3) as f32;
    let weak = m.health / m.max_health() < 0.5;
    if (r < damage || weak)
        && damage < m.health
        && m.in_water
        && by.is_some()
        && !m.bm.playing_dead
        && let Some(b) = m.bm.brain.as_mut()
    {
        b.mem.play_dead.set(200);
    }
}

/// Air: out of water and rain, −1 per tick, drying out at −20.
pub fn handle_air(s: &mut Server, m: &mut Mob, before: i32) {
    if m.no_ai {
        return;
    }
    if m.alive() && !s.mob_in_water_or_rain(m) {
        m.air = before - 1;
        if m.air <= -20 {
            m.air = 0;
            s.mob_hurt(m, "minecraft:dry_out", 2.0, None, None);
        }
    } else {
        m.air = MAX_AIR;
    }
}

/// End of combat: target killed by a player within 20 blocks → support effects.
pub fn on_stop_attacking(s: &mut Server, m: &Mob, target: Ent) {
    let Ent::Mob(id) = target else { return };
    let Some(o) = s.mob_ref(id) else { return };
    if o.health > 0.0 && !o.dead {
        return;
    }
    if o.last_hit.as_ref().and_then(|h| h.by) != Some(Ent::Player) {
        return;
    }
    let bb = m.bbox();
    let area = rc_physics::Aabb { min: bb.min.map(|v| v - 20.0), max: bb.max.map(|v| v + 20.0) };
    if s.player_aabb().intersects(&area) {
        supporting_effects(s, m);
    }
}

/// Regeneration I (100 + remaining duration, 2,400 at most) if it is missing or ends in under
/// 2,399 ticks, given by the axolotl (trigger source); mining fatigue removed.
fn supporting_effects(s: &mut Server, m: &Mob) {
    let cur = s.fx().get("minecraft:regeneration").map(|e| e.dur);
    let ends_soon = cur.is_none_or(|d| d != -1 && d <= 2399);
    if ends_soon {
        let prev = cur.unwrap_or(0).max(0);
        s.give_player_effect_from("minecraft:regeneration", (100 + prev).min(2400), 0, Some(m));
    }
    s.remove_effect("minecraft:mining_fatigue");
}

/// Bucket data: variant (component), age, lock, remaining hunt cooldown.
pub fn bucket_data(m: &Mob, b: &mut Stack, data: &mut Nbt) {
    let v = VARIANTS.get(m.variant as usize).copied().unwrap_or("lucy");
    crate::items::set_component(b, "minecraft:axolotl/variant", Nbt::String(v.into()));
    nbtio::set(data, "Age", Nbt::Int(m.age));
    nbtio::set(data, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked)));
    if let Some(br) = m.bm.brain.as_ref()
        && br.mem.hunting_cd.has()
    {
        nbtio::set(data, "HuntingCooldown", Nbt::Long(br.mem.hunting_cd.ttl));
    }
}

pub fn load_bucket_data(m: &mut Mob, bucket: &Stack, data: &Nbt) {
    if let Some(v) = crate::items::component(bucket, "minecraft:axolotl/variant").and_then(Nbt::as_str)
        && let Some(i) = VARIANTS.iter().position(|x| *x == v.trim_start_matches("minecraft:"))
    {
        m.variant = i as i32;
    }
    m.age = nbtio::int(data, "Age").unwrap_or(0) as i32;
    m.mount.age_locked = nbtio::int(data, "AgeLocked").is_some_and(|v| v != 0);
    if let Some(b) = m.bm.brain.as_mut() {
        match data.get("HuntingCooldown").and_then(Nbt::as_i64) {
            Some(t) => b.mem.hunting_cd.set_exp(true, t),
            None => b.mem.hunting_cd.erase(),
        }
    }
}

/// Water bucket (pickup), otherwise animal (tropical fish bucket returned as a water bucket).
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if let Some(r) = super::bucket_pickup(s, m, slot, item) {
        return Some(r);
    }
    s.bz_animal_interact(m, slot, item)
}
