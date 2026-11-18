//! Proof of the kind test by facts (`RUSTCRAFT_OPT_KIND_ID_FILTERS`): over every type of the
//! registry, the movement family of the facts is that of the type name; two worlds of 1,000 mixed
//! entities ticked 200 times, one by facts and one by names, stay equal bit for bit; a sabotaged
//! fact is caught.

use super::*;
use crate::entities::KindFacts;
use rc_client::EntityEvent;
use rc_proto_v777::entity::{AddEntity, Meta};

/// Fixed generator (xorshift 64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn coord(&mut self, r: f64) -> f64 {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * r
    }
}

/// Every simulated projectile, and entities that throw, hook or carry them.
const KINDS: &[&str] = &[
    "arrow",
    "spectral_arrow",
    "trident",
    "snowball",
    "egg",
    "ender_pearl",
    "splash_potion",
    "lingering_potion",
    "experience_bottle",
    "fireball",
    "small_fireball",
    "dragon_fireball",
    "wither_skull",
    "wind_charge",
    "breeze_wind_charge",
    "llama_spit",
    "shulker_bullet",
    "eye_of_ender",
    "firework_rocket",
    "fishing_bobber",
    "cow",
    "zombie",
    "drowned",
    "player",
    "item",
    "oak_boat",
];

/// Local player of the worlds.
const ME: i32 = 900_000;

fn data() -> rc_data::VersionData {
    rc_testdata::version_data().expect("version data missing (extraction)")
}

/// Stone under y = 64, air above.
struct Floor {
    stone: u32,
}

impl rc_physics::World for Floor {
    fn state(&self, _: i32, y: i32, _: i32) -> Option<u32> {
        Some(if y < 64 { self.stone } else { 0 })
    }

    fn collision(&self, _: i32, y: i32, _: i32, out: &mut Vec<rc_physics::Aabb>) -> bool {
        if y < 64 {
            out.push(rc_physics::Aabb::UNIT);
        }
        false
    }
}

/// `count` mixed entities, the same for a given seed.
fn mixed_world(data: &rc_data::VersionData, seed: u64, count: i32) -> Entities {
    let names = data.registry("minecraft:entity_type").expect("type registry").entries.clone();
    let mut ents = Entities { type_names: names.clone(), dims: data.entity_dims.clone(), local: ME, ..Default::default() };
    let mut rng = Rng(seed);
    let id_of = |k: &str| names.iter().position(|n| n.strip_prefix("minecraft:") == Some(k)).expect(k) as i32;
    for id in 1..=count {
        let kind = KINDS[rng.below(KINDS.len())];
        let pos = [rng.coord(20.0), 64.0 + rng.coord(6.0).abs(), rng.coord(20.0)];
        let velocity = [rng.coord(0.8), rng.coord(0.5), rng.coord(0.8)];
        let owner = if rng.below(3) == 0 { ME } else { 1 + rng.below(count as usize) as i32 };
        let add = AddEntity {
            id,
            uuid: 0x9000 + id as u128,
            kind: id_of(kind),
            pos,
            velocity,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: owner,
        };
        ents.apply(EntityEvent::Add(Box::new(add)));
        let mut values = vec![(5, Meta::Bool(rng.below(5) == 0))];
        match kind {
            "trident" => {
                values.push((11, Meta::Byte(rng.below(4) as i8)));
                values.push((8, Meta::Byte([0, 1, 2, 3][rng.below(4)])));
            }
            "arrow" | "spectral_arrow" => values.push((8, Meta::Byte([0, 1, 2][rng.below(3)]))),
            "fishing_bobber" => values.push((8, Meta::Int(if rng.below(4) == 0 { 0 } else { owner + 1 }))),
            "firework_rocket" => {
                let attached = (rng.below(3) == 0).then_some(owner);
                values.push((9, Meta::OptionalInt(attached)));
                values.push((10, Meta::Bool(rng.below(2) == 0)));
            }
            _ => {}
        }
        ents.apply(EntityEvent::Data { id, values });
    }
    // A few passengers: skipped by the simulation.
    for v in (1..count).step_by(97) {
        ents.apply(EntityEvent::Passengers { vehicle: v, passengers: vec![v + 1] });
    }
    ents
}

/// An entity as bits: id, position and velocity, rotation, stuck flag, simulated body.
type EntityBits = (i32, [u64; 6], [u32; 2], bool, bool);

/// Every entity as bits, by id.
fn snapshot(ents: &Entities) -> Vec<EntityBits> {
    let mut out: Vec<_> = ents
        .map
        .iter()
        .map(|(&id, e)| {
            let pv = [e.pos[0], e.pos[1], e.pos[2], e.velocity[0], e.velocity[1], e.velocity[2]];
            (id, pv.map(f64::to_bits), [e.yaw.to_bits(), e.pitch.to_bits()], e.meta_bool(10), e.motion.is_some())
        })
        .collect();
    out.sort_unstable_by_key(|s| s.0);
    out
}

/// Ticks at which the world ticked by facts (`sabotage` applied to it first) differs from the
/// world ticked by names.
fn differing_ticks(sabotage: impl Fn(&mut Entities)) -> usize {
    let data = data();
    let phys = rc_physics::Tables::new(&data);
    let stone = data.blocks.iter().find(|b| b.name == "minecraft:stone").expect("stone").default_state;
    let floor = Floor { stone };
    let power = hashbrown::HashMap::new();
    let mut by_names = mixed_world(&data, 21, 1_000);
    let mut by_facts = mixed_world(&data, 21, 1_000);
    sabotage(&mut by_facts);
    let mut ps = [0, 1].map(|_| Particles::new(crate::particles::tests::assets(), &data));
    let mut seeds = [7u64, 7u64];
    let mut differ = 0;
    for t in 0..200 {
        let me = Follow { id: ME, pos: [0.5 + f64::from(t) * 0.05, 64.0, 0.5], eye: 1.62, spectator: false, dead: false };
        let [p0, p1] = &mut ps;
        let [s0, s1] = &mut seeds;
        tick_projectiles(&mut by_names, &phys, &floor, me, 1000 + i64::from(t), false, &power, p0, s0, false);
        tick_projectiles(&mut by_facts, &phys, &floor, me, 1000 + i64::from(t), false, &power, p1, s1, true);
        // Trails are not compared, and never ticked here: dropped each tick.
        p0.clear();
        p1.clear();
        // The server removes what flies away (a rocket gains 15 % of speed per tick until it
        // explodes): beyond 300 blocks, gone from both worlds alike.
        for ents in [&mut by_names, &mut by_facts] {
            ents.map.retain(|_, e| e.pos.iter().all(|v| v.abs() < 300.0));
        }
        differ += usize::from(snapshot(&by_names) != snapshot(&by_facts) || seeds[0] != seeds[1]);
    }
    // Guard: the proof must have simulated a crowd of projectiles to the end.
    let simulated = by_names.map.values().filter(|e| e.motion.is_some()).count();
    assert!(simulated > 300, "too few projectiles simulated to the end: {simulated}");
    differ
}

#[test]
fn projectiles_tick_the_same_by_facts_and_by_names() {
    assert_eq!(differing_ticks(|_| {}), 0);
}

/// Sabotage: snowballs, eggs and pearls given the potions' gravity are caught.
#[test]
fn sabotaged_motion_fact_is_caught() {
    let heavier = |ents: &mut Entities| {
        for e in ents.map.values_mut().filter(|e| e.facts.motion == MotionFamily::ThrownLight) {
            e.facts = KindFacts { motion: MotionFamily::ThrownPotion, ..e.facts };
        }
    };
    assert!(differing_ticks(heavier) > 0);
}

/// Movement family of every type of the 26.3 registry: facts and names agree.
#[test]
fn motion_family_of_every_registry_type_matches_the_names() {
    let data = data();
    let names = &data.registry("minecraft:entity_type").expect("type registry").entries;
    assert!(names.len() > 100, "type registry too short: {}", names.len());
    let mismatched: Vec<&str> = names
        .iter()
        .map(|n| n.strip_prefix("minecraft:").unwrap_or(n))
        .filter(|k| family_of_motion(KindFacts::of(k).motion) != family(k))
        .collect();
    assert!(mismatched.is_empty(), "{mismatched:?}");
    let projectiles = names.iter().filter(|n| family(n.strip_prefix("minecraft:").unwrap_or(n)).is_some()).count();
    assert_eq!(projectiles, 20, "simulated projectile types");
}
