//! Entity type tags frozen into lists in the code, compared species by species with the
//! jar's tags (version data required, otherwise the test passes vacuously).

use super::species::Kind;

type Pred = fn(Kind) -> bool;

/// Jar tag → predicate that must match it exactly across all simulated species.
const CHECKS: &[(&str, Pred)] = &[
    ("minecraft:fall_damage_immune", |k| k.info().fall_immune),
    ("minecraft:can_breathe_under_water", Kind::breathes_underwater),
    ("minecraft:undead", Kind::undead),
    ("minecraft:sensitive_to_smite", Kind::undead),
    ("minecraft:arthropod", Kind::arthropod),
    ("minecraft:sensitive_to_bane_of_arthropods", Kind::arthropod),
    ("minecraft:aquatic", Kind::aquatic),
    ("minecraft:sensitive_to_impaling", Kind::aquatic),
    ("minecraft:not_scary_for_pufferfish", super::sealife::fish::not_scary),
    ("minecraft:dismounts_underwater", super::mounts::dismounts_underwater),
    ("minecraft:no_anger_from_wind_charge", Kind::no_anger_from_wind_charge),
    ("minecraft:non_controlling_rider", Kind::non_controlling_rider),
    ("minecraft:can_float_while_ridden", Kind::floats_while_ridden),
    ("minecraft:cannot_be_age_locked", Kind::cannot_be_age_locked),
    ("minecraft:powder_snow_walkable_mobs", Kind::powder_snow_walkable),
    ("minecraft:freeze_immune_entity_types", Kind::freeze_immune),
    ("minecraft:freeze_hurts_extra_types", Kind::freeze_hurts_extra),
    ("minecraft:skeletons", Kind::skeletons_tag),
    ("minecraft:raiders", super::illagers::is_raider),
    ("minecraft:burn_in_daylight", |k| k.info().burns),
    ("minecraft:ignores_poison_and_regen", Kind::undead),
    ("minecraft:inverted_healing_and_harm", Kind::undead),
];

#[test]
fn entity_tags_match_jar() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(jar) = crate::registries::client_jar(&data) else { return };
    let Ok(jar) = crate::registries::Jar::read(&jar) else { return };
    let regs = crate::registries::load_jar(&jar, &data);
    let Some(types) = data.registry("minecraft:entity_type") else { return };
    let Some((_, tags)) = regs.tags.iter().find(|(r, _)| r == "minecraft:entity_type") else {
        panic!("entity type tags absent");
    };
    let mut wrong = Vec::new();
    for (tag, pred) in CHECKS {
        let ids = &tags.iter().find(|(n, _)| n == tag).unwrap_or_else(|| panic!("tag {tag} absent")).1;
        for k in Kind::ALL {
            let Some(idx) = types.entries.iter().position(|n| n == k.info().name) else { continue };
            let want = ids.contains(&(idx as i32));
            if pred(k) != want {
                wrong.push(format!("{tag}: {k:?} expected {want}"));
            }
        }
    }
    eprintln!("{} tags compared over {} species", CHECKS.len(), Kind::ALL.len());
    assert!(wrong.is_empty(), "differences:\n{}", wrong.join("\n"));
}

/// Creeper table: one disc drawn from the tag when the damage's attacker is a skeleton, none
/// otherwise (`entity_properties` condition on `attacker`, tag entry expanded).
#[test]
fn creeper_disc_needs_skeleton_attacker() {
    let Some(data) = rc_testdata::version_data() else { return };
    let data = std::sync::Arc::new(data);
    let Some(jar) = crate::registries::client_jar(&data) else { return };
    let Ok(jar) = crate::registries::Jar::read(&jar) else { return };
    let regs = crate::registries::load_jar(&jar, &data);
    let pack = crate::datapack::DataPack::from_jar(&jar);
    let items = crate::items::Items::new(data.clone(), &regs.tags);
    let bt = |_: &str, _: &str| false;
    let loot = crate::loot::Loot { pack: &pack, items: &items, block_tag: &bt };
    let types = data.registry("minecraft:entity_type").expect("types");
    let tags = &regs.tags.iter().find(|(r, _)| r == "minecraft:entity_type").expect("tags").1;
    let tags_of = |name: &str| -> Vec<String> {
        let idx = types.entries.iter().position(|n| n == name).expect("type") as i32;
        tags.iter().filter(|(_, ids)| ids.contains(&idx)).map(|(n, _)| n.clone()).collect()
    };
    let discs = |attacker: &str| -> Vec<String> {
        let ctx = crate::loot::Ctx {
            damage_type: Some("minecraft:arrow".into()),
            attacker: Some((attacker.to_owned(), tags_of(attacker))),
            ..Default::default()
        };
        let mut rng = crate::rng::Rng::new(42);
        let mut out = Vec::new();
        for _ in 0..20 {
            for s in loot.drops("minecraft:entities/creeper", &ctx, &mut rng) {
                if s.id.contains("music_disc") {
                    out.push(s.id);
                }
            }
        }
        out
    };
    let from_skeleton = discs("minecraft:skeleton");
    eprintln!("discs (skeleton): {from_skeleton:?}");
    assert_eq!(from_skeleton.len(), 20, "one disc per death: {from_skeleton:?}");
    let distinct: hashbrown::HashSet<_> = from_skeleton.iter().collect();
    assert!(distinct.len() > 1, "a single disc drawn from the tag, not the whole tag: {distinct:?}");
    assert!(discs("minecraft:zombie").is_empty());
}

/// Attributes: base and permanent modifiers (spawn bonus, `/attribute`) kept on
/// save and re-read; transient modifiers (effects) dropped; unmodelled attributes
/// copied.
#[test]
fn attributes_round_trip() {
    use rc_proto::nbt::Nbt;
    let rng = super::piglins::test_rng();
    let mut m = super::mob::Mob::new(Kind::Zombie, 1, 7, [0.0; 3], 0.0, rng);
    m.attrs.max_health.set_perm_mod("minecraft:leader_zombie_bonus", 1.5, 2);
    m.attrs.safe_fall.base = 10.0;
    m.attrs.safe_fall.saved = true;
    m.attrs.speed.set_mod("minecraft:effect.speed", 0.2, 2);
    m.attrs.extra.push(Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:scale".into())),
        ("base".into(), Nbt::Double(2.0)),
    ]));
    let e = m.save();
    let mut n = super::mob::Mob::new(Kind::Zombie, 2, 8, [0.0; 3], 0.0, super::piglins::test_rng());
    n.load(&e);
    assert_eq!(n.attrs.max_health.mods, vec![("minecraft:leader_zombie_bonus", 1.5, 2)]);
    assert!(n.attrs.max_health.perm.contains(&"minecraft:leader_zombie_bonus"));
    assert_eq!(n.attrs.safe_fall.base, 10.0);
    assert!(n.attrs.speed.mods.is_empty(), "transient effect not recorded");
    assert_eq!(n.attrs.extra.len(), 1);
    assert!((n.max_health() - 50.0).abs() < 1e-4);
}

/// Adult goat re-read: the recorded damage base stays (only a kid is reset to 1).
#[test]
fn adult_goat_keeps_saved_attack() {
    let mut g = super::mob::Mob::new(Kind::Goat, 1, 7, [0.0; 3], 0.0, super::piglins::test_rng());
    g.attrs.attack.base = 9.0;
    g.attrs.attack.saved = true;
    let e = g.save();
    let mut n = super::mob::Mob::new(Kind::Goat, 2, 8, [0.0; 3], 0.0, super::piglins::test_rng());
    n.load(&e);
    assert_eq!(n.attrs.attack.base, 9.0);
    let mut b = super::mob::Mob::new(Kind::Goat, 3, 9, [0.0; 3], 0.0, super::piglins::test_rng());
    b.attrs.attack.base = 9.0;
    b.attrs.attack.saved = true;
    b.age = -24000;
    let e = b.save();
    let mut nb = super::mob::Mob::new(Kind::Goat, 4, 10, [0.0; 3], 0.0, super::piglins::test_rng());
    nb.load(&e);
    assert_eq!(nb.attrs.attack.base, 1.0);
}

/// Wolf re-read without an owner: 8 max HP base whatever the data.
#[test]
fn ownerless_wolf_resets_max_health() {
    let mut w = super::mob::Mob::new(Kind::Wolf, 1, 7, [0.0; 3], 0.0, super::piglins::test_rng());
    w.attrs.max_health.base = 40.0;
    w.health = 40.0;
    let e = w.save();
    let mut n = super::mob::Mob::new(Kind::Wolf, 2, 8, [0.0; 3], 0.0, super::piglins::test_rng());
    n.load(&e);
    assert_eq!(n.attrs.max_health.base, 8.0);
    assert!(n.health <= 8.0);
}

/// A mounted cube does not steer its mount (`non_controlling_rider`).
#[test]
fn cube_rider_does_not_control() {
    use super::mob::Ent;
    let mut pig = super::mob::Mob::new(Kind::Pig, 1, 7, [0.0; 3], 0.0, super::piglins::test_rng());
    pig.mount.passengers.push(Ent::Mob(5));
    assert_eq!(crate::server::Server::mob_controller(&pig), Some(5));
    pig.mount.passive_riders.push(5);
    assert_eq!(crate::server::Server::mob_controller(&pig), None);
}
