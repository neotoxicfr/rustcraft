//! Data the spawning reads from the pack, on the platform of the test world
//! (`RUSTCRAFT_SOLO_WORLD`): the frog variant chosen by the biome, the rivers' tag that rarefies
//! the water ambient spawns, the enchantment provider of the spawn equipment.

use rc_proto::nbt::Nbt;

use super::jrand::JRand;
use super::spawn::{Group, REDUCED_WATER_AMBIENT_SPAWNS, Reason, mob_spawn_enchant};
use super::species::Kind;
use super::tests_spawn_paths::open;
use crate::blocks::probe::Probe;
use crate::player::Stack;

/// The middle of the platform, far from the borders of the biome set around it.
const MIDDLE: [i32; 3] = [216, -59, 216];

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// The platform (200..231) and the air above it set to `biome`.
fn platform_biome(p: &mut Probe, biome: &str) {
    cmd(p, &format!("fillbiome 200 -64 200 231 -49 231 {biome}"));
}

/// A frog finalised in a biome of the warm frogs' tag is warm, in one of the cold frogs' tag
/// cold, elsewhere temperate: the variants' spawn conditions come from the pack.
#[test]
fn frog_variant_follows_the_biome() {
    let Some((mut p, _dir)) = open("frog-variant-biome") else { return };
    let at = MIDDLE.map(|c| f64::from(c) + 0.5);
    for (biome, variant) in [
        ("minecraft:mangrove_swamp", "minecraft:warm"),
        ("minecraft:snowy_plains", "minecraft:cold"),
        ("minecraft:plains", "minecraft:temperate"),
    ] {
        platform_biome(&mut p, biome);
        let mut frog = p.s.new_mob(Kind::Frog, [at[0], f64::from(MIDDLE[1]), at[2]], 0.0);
        p.s.finalize_spawn(&mut frog, Reason::Natural, &mut Group::default());
        assert_eq!(frog.bm.frog_variant.as_deref(), Some(variant), "frog finalised in {biome}");
        let index = p.s.regs.entries("minecraft:frog_variant").iter().position(|v| v == variant);
        assert_eq!(Some(frog.variant as usize), index, "network variant of the frog in {biome}");
    }
}

/// The tag that refuses 98 % of the water ambient spawn attempts is the game's file: a river is
/// in it, an ocean is not.
#[test]
fn rivers_are_in_the_reduced_water_ambient_tag() {
    let Some((mut p, _dir)) = open("river-ambient-tag") else { return };
    platform_biome(&mut p, "minecraft:river");
    assert!(p.s.biome_tag(MIDDLE, REDUCED_WATER_AMBIENT_SPAWNS), "a river is in the tag");
    platform_biome(&mut p, "minecraft:frozen_river");
    assert!(p.s.biome_tag(MIDDLE, REDUCED_WATER_AMBIENT_SPAWNS), "a frozen river is in the tag");
    platform_biome(&mut p, "minecraft:ocean");
    assert!(!p.s.biome_tag(MIDDLE, REDUCED_WATER_AMBIENT_SPAWNS), "an ocean is not");
}

/// Seeds of the spawn equipment draw below: every one gives the sword at least one enchantment
/// (the cost is 5 or more and the tag holds every non-treasure enchantment).
const SEEDS: i64 = 64;

/// The spawn equipment's enchantment provider draws among the tag `on_mob_spawn_equipment`, read
/// from the pack: a diamond sword enchanted at the highest special multiplier always gets at
/// least one enchantment (the tag was read empty: never), and the draws stay those of the game's
/// provider (the cost first, then the table's draw).
#[test]
fn spawn_equipment_gets_enchanted() {
    let Some((p, _dir)) = open("spawn-equipment-enchanted") else { return };
    let mut enchanted = 0;
    let mut seen: Vec<String> = Vec::new();
    for seed in 0..SEEDS {
        let mut rng = JRand::new(seed);
        let sword = Stack { id: "minecraft:diamond_sword".into(), count: 1, components: None };
        let out = mob_spawn_enchant(&p.s.life.pack, &p.s.life.items, sword, 1.0, &mut rng);
        let Some(Nbt::Compound(levels)) = crate::items::component(&out, "minecraft:enchantments") else { continue };
        enchanted += usize::from(!levels.is_empty());
        for (id, _) in levels {
            if !seen.contains(id) {
                seen.push(id.clone());
            }
        }
    }
    seen.sort();
    eprintln!("spawn equipment: {enchanted} of {SEEDS} swords enchanted, with {seen:?}");
    assert_eq!(enchanted, SEEDS as usize, "every sword enchanted");
    assert!(seen.contains(&"minecraft:sharpness".to_owned()), "sharpness drawn: {seen:?}");
    assert!(!seen.iter().any(|e| e == "minecraft:mending"), "a treasure enchantment drawn: {seen:?}");
}
