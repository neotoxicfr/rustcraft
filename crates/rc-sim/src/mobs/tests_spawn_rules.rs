//! Natural spawns, spawn eggs and spawner blocks checked against the rules of the game: where a
//! group may start and land (world spawn, world border, copper grates, per-player cap), what a
//! spawn egg hands to its mob, what a monster spawner can spawn, which biome and which ambient
//! light a mob reads.

use std::time::Instant;

use rc_proto::nbt::Nbt;

use super::mob::{Ent, Mob};
use super::recent::vibration::Src;
use super::spawn::PlayerMobCaps;
use super::species::{Category, Kind};
use super::tests_spawn_paths::{add_guest, open};
use crate::blocks::Dir;
use crate::blocks::entities::EntKind;
use crate::blocks::probe::Probe;
use crate::nbtio;
use crate::player::Stack;

/// On the stone platform of the test area (200..231 at y = -60), near its north edge.
const ON_PLATFORM: &str = "player 216.5 -59 206.5 0 0\n";
/// North of the platform: each of its blocks is between 24 and 128 blocks away, where natural
/// groups may land.
const NORTH_OF_PLATFORM: &str = "player 216.5 -59 170.5 0 0\n";
/// A block of the platform's floor, and the cell above it.
const FLOOR: [i32; 3] = [216, -60, 216];
const ABOVE_FLOOR: [i32; 3] = [216, -59, 216];
const SPAWNER: [i32; 3] = [216, -59, 212];

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn night_with_natural_spawns(p: &mut Probe) {
    nbtio::set(&mut p.s.rules, "minecraft:spawn_mobs", Nbt::Byte(1));
    cmd(p, "time set midnight");
}

/// Natural spawn passes without any mob tick (the mobs stay where they spawned). With
/// `uncounted`, every mob is made persistent after each pass: the caps never fill.
fn spawn_passes(p: &mut Probe, passes: u32, uncounted: bool) {
    for _ in 0..passes {
        p.s.time += 1;
        p.s.natural_spawn_tick();
        if uncounted {
            for m in p.s.mobs.list.iter_mut() {
                m.persistent = true;
            }
        }
    }
}

fn on_platform(m: &Mob) -> bool {
    let p = m.block_pos();
    p[1] == -59 && (200..=231).contains(&p[0]) && (200..=231).contains(&p[2])
}

fn mobs_on_platform(p: &Probe) -> usize {
    p.s.mobs.list.iter().filter(|m| on_platform(m)).count()
}

fn egg(item: &str, components: Vec<(&str, Nbt)>) -> Stack {
    let components = components.into_iter().map(|(key, value)| (key.to_owned(), value)).collect::<Vec<_>>();
    Stack { id: item.into(), count: 1, components: (!components.is_empty()).then_some(Nbt::Compound(components)) }
}

fn compound(fields: Vec<(&str, Nbt)>) -> Nbt {
    Nbt::Compound(fields.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// The mobs added since `before` (their UUIDs).
fn new_mobs<'a>(p: &'a Probe, before: &[u128]) -> Vec<&'a Mob> {
    p.s.mobs.list.iter().filter(|m| !before.contains(&m.uuid)).collect()
}

fn mob_uuids(p: &Probe) -> Vec<u128> {
    p.s.mobs.list.iter().map(|m| m.uuid).collect()
}

/// The "entity placed" game events emitted since the log was cleared: position and source.
fn entity_place_events(p: &Probe) -> Vec<([f64; 3], Option<Src>)> {
    let log = &p.s.mobs.rec.vib.log;
    log.iter().filter(|event| event.0 == "minecraft:entity_place").map(|event| (event.1, event.2)).collect()
}

/// The custom data a stack handed to its mob.
fn custom_data_of(m: &Mob) -> Option<&Nbt> {
    m.nbt.as_ref().and_then(|kept| kept.get("data"))
}

fn biome_index(p: &Probe, name: &str) -> usize {
    let biomes = p.s.regs.entries("minecraft:worldgen/biome");
    biomes.iter().position(|biome| biome == name).unwrap_or_else(|| panic!("{name} is not a biome"))
}

/// Around the platform, two blocks the zoom moves across the border of `biome`: one whose own
/// stored cell holds `biome` while the block reads as another biome, and one in the opposite
/// case.
fn blocks_moved_across_the_border_of(p: &Probe, biome: usize) -> ([i32; 3], [i32; 3]) {
    let (mut read_as_another, mut read_as_this) = (None, None);
    for x in 196..232 {
        for z in 196..232 {
            for y in -62..-54 {
                let block = [x, y, z];
                let (stored, zoomed) = (p.s.stored_biome_at(block), p.s.biome_at(block));
                if stored == Some(biome) && zoomed != Some(biome) {
                    read_as_another.get_or_insert(block);
                }
                if stored != Some(biome) && zoomed == Some(biome) {
                    read_as_this.get_or_insert(block);
                }
            }
        }
    }
    (read_as_another.expect("a block of the biome read as another"), read_as_this.expect("a block read as the biome"))
}

/// For every block of a box: the biome read (always zoomed, and with the shortcut) is the
/// stored biome of the cell the zoom picks. Returns how many blocks the shortcut served.
fn assert_biome_reads_match_the_zoom(
    p: &Probe,
    horizontal: std::ops::Range<i32>,
    heights: std::ops::RangeInclusive<i32>,
) -> usize {
    let seed = p.s.wgs.zoom_seed;
    let mut without_zoom = 0;
    for x in horizontal.clone() {
        for z in horizontal.clone() {
            for y in heights.clone() {
                let block = [x, y, z];
                let (qx, qy, qz) = rc_worldgen::biome::zoom(seed, x, y, z);
                let through_zoom = p.s.stored_biome_at([qx << 2, qy << 2, qz << 2]);
                assert_eq!(p.s.biome_at_with(block, false), through_zoom, "always zoomed, {block:?}");
                assert_eq!(p.s.biome_at_with(block, true), through_zoom, "with the shortcut, {block:?}");
                without_zoom += usize::from(p.s.biome_of_uniform_sections_around(block).is_some());
            }
        }
    }
    without_zoom
}

// ---------- Where a natural group may start and land ----------

/// None of the eight copper grates holds a spawn, though each is a full block.
#[test]
fn copper_grates_never_hold_a_spawn() {
    let Some((mut p, _dir)) = open("spawn-rules-grates") else { return };
    assert!(p.s.valid_spawn_on(FLOOR, Kind::Zombie), "stone holds a spawn");
    for stage in ["", "exposed_", "weathered_", "oxidized_"] {
        for wax in ["", "waxed_"] {
            let grate = format!("minecraft:{wax}{stage}copper_grate");
            cmd(&mut p, &format!("setblock 216 -60 216 {grate}"));
            assert!(!p.s.valid_spawn_on(FLOOR, Kind::Zombie), "{grate} holds a spawn");
        }
    }
}

/// A group is abandoned when its start block conducts redstone, not when it is merely full and
/// opaque: a redstone block lets the group start.
#[test]
fn group_start_is_blocked_by_redstone_conductors_only() {
    let Some((mut p, _dir)) = open("spawn-rules-group-start") else { return };
    assert!(p.s.blocks_spawn_group_start(FLOOR), "stone conducts");
    assert!(!p.s.blocks_spawn_group_start(ABOVE_FLOOR), "air");
    cmd(&mut p, "setblock 216 -60 216 minecraft:redstone_block");
    assert!(!p.s.blocks_spawn_group_start(FLOOR), "a redstone block does not conduct");
}

/// No natural spawn within 24 blocks of the centre of the world spawn block: the whole platform
/// lies inside that sphere once the world spawn is at its centre.
#[test]
fn no_natural_spawn_within_24_blocks_of_the_world_spawn() {
    let Some((mut p, _dir)) = open("spawn-rules-world-spawn") else { return };
    p.run(NORTH_OF_PLATFORM);
    night_with_natural_spawns(&mut p);
    p.s.level.spawn_dimension = p.s.dim.name.clone();
    p.s.level.spawn_pos = [5000, -59, 5000];
    spawn_passes(&mut p, 300, true);
    let far_from_world_spawn = mobs_on_platform(&p);
    assert!(far_from_world_spawn >= 10, "the platform spawns mobs: {far_from_world_spawn}");
    p.s.level.spawn_pos = ABOVE_FLOOR;
    spawn_passes(&mut p, 600, true);
    assert_eq!(mobs_on_platform(&p), far_from_world_spawn, "spawns within 24 blocks of the world spawn");
    // The sphere belongs to the dimension players spawn in.
    p.s.level.spawn_dimension = "minecraft:the_nether".into();
    spawn_passes(&mut p, 300, true);
    assert!(mobs_on_platform(&p) > far_from_world_spawn, "world spawn of another dimension");
}

/// Natural groups only land in chunks wholly inside the world border, and a restricted
/// placement refuses a block outside it.
#[test]
fn natural_spawns_stay_inside_the_world_border() {
    let Some((mut p, _dir)) = open("spawn-rules-border") else { return };
    p.run(NORTH_OF_PLATFORM);
    night_with_natural_spawns(&mut p);
    // Border x 176..256, z 168..248: chunks 11..=15 by 11..=14 are wholly inside.
    p.s.cmd.border.center = [216.0, 208.0];
    p.s.cmd.border.size = 80.0;
    let before = mob_uuids(&p);
    spawn_passes(&mut p, 300, true);
    let spawned = new_mobs(&p, &before);
    assert!(spawned.len() >= 20, "mobs spawned inside the border: {}", spawned.len());
    for m in &spawned {
        let b = m.block_pos();
        let chunk = (b[0] >> 4, b[2] >> 4);
        assert!((11..=15).contains(&chunk.0) && (11..=14).contains(&chunk.1), "{:?} spawned at {b:?}", m.kind);
    }
    // Placement alone (chunk generation, reinforcements): ground refused outside the border,
    // an unrestricted species (fox) accepted anywhere.
    assert!(p.s.spawn_placement_ok(Kind::Zombie, ABOVE_FLOOR));
    p.s.cmd.border.center = [1000.0, 1000.0];
    assert!(!p.s.spawn_placement_ok(Kind::Zombie, ABOVE_FLOOR), "ground placement outside the border");
    assert!(p.s.spawn_placement_ok(Kind::Fox, ABOVE_FLOOR), "no restriction: no border test");
}

/// The local cap: a mob counts for the non-spectator players within 128 blocks of its chunk, and
/// a chunk spawns a category while one of the players near it is under the cap.
#[test]
fn local_cap_counts_mobs_per_player() {
    let near_origin = [0.0, 64.0, 0.0];
    let east = [200.0, 64.0, 0.0];
    let mut caps = PlayerMobCaps::new(&[(near_origin, false), (east, false), ([0.0, 64.0, 2000.0], true)]);
    let monsters = Category::Monster.max();
    for _ in 0..monsters {
        caps.add_mob((0, 0), Category::Monster);
    }
    assert!(!caps.can_spawn(Category::Monster, (0, 0)), "the only player near is at the cap");
    assert!(caps.can_spawn(Category::Creature, (0, 0)), "another category has room");
    // Chunk (6, 0), centre x = 104: near both players, and the eastern one has room.
    assert!(caps.can_spawn(Category::Monster, (6, 0)), "a second player near has room");
    assert!(caps.can_spawn(Category::Monster, (14, 0)), "near the eastern player only");
    assert!(!caps.can_spawn(Category::Monster, (40, 0)), "no player near");
    assert!(!caps.can_spawn(Category::Monster, (0, 125)), "a spectator does not count");
    for _ in 0..monsters {
        caps.add_mob((6, 0), Category::Monster);
    }
    assert!(!caps.can_spawn(Category::Monster, (6, 0)), "both players at the cap");
}

/// A guest surrounded by a full cap of monsters far away does not stop the spawns around the
/// host: each player has its own count.
#[test]
fn far_player_at_the_cap_does_not_stop_spawns_near_the_host() {
    let Some((mut p, _dir)) = open("spawn-rules-local-cap") else { return };
    p.run(NORTH_OF_PLATFORM);
    night_with_natural_spawns(&mut p);
    let _guest = add_guest(&mut p.s, "Guest", [816.5, -59.0, 206.5]);
    for i in 0..Category::Monster.max() {
        let at = [810.5 + f64::from(i % 10), -59.0, 200.5 + f64::from(i / 10)];
        let zombie = p.s.new_mob(Kind::Zombie, at, 0.0);
        p.s.add_mob(zombie);
    }
    spawn_passes(&mut p, 300, false);
    let near_host = p.s.mobs.list.iter().filter(|m| m.kind.is_monster() && m.pos()[0] < 500.0).count();
    assert!(near_host > 0, "monsters spawn around the host");
}

/// Natural groups are drawn around every player of a LAN game: a guest standing north of the
/// platform gets spawns there while the host is far away.
#[test]
fn natural_groups_spawn_around_a_lan_guest_far_from_the_host() {
    let Some((mut p, _dir)) = open("spawn-rules-guest-groups") else { return };
    p.run(NORTH_OF_PLATFORM);
    night_with_natural_spawns(&mut p);
    let _guest = add_guest(&mut p.s, "Guest", [216.5, -59.0, 170.5]);
    p.s.pos = [1216.5, -59.0, 170.5];
    spawn_passes(&mut p, 300, true);
    let around_the_guest = mobs_on_platform(&p);
    assert!(around_the_guest >= 10, "mobs spawned around the guest: {around_the_guest}");
}

/// A mob despawns by its distance to the nearest player who is not a spectator: near a guest it
/// stays though the host is far, and a spectator alone never despawns anything.
#[test]
fn despawn_measures_from_the_nearest_player_who_is_not_a_spectator() {
    let Some((mut p, _dir)) = open("spawn-rules-guest-despawn") else { return };
    let mut zombie = p.s.new_mob(Kind::Zombie, [216.5, -59.0, 216.5], 0.0);
    p.s.pos = [216.5, -59.0, 616.5];
    assert!(p.s.check_despawn(&mut zombie), "400 blocks from the only player");

    let _guest = add_guest(&mut p.s, "Guest", [216.5, -59.0, 236.5]);
    zombie.no_action_time = 900;
    assert!(!p.s.check_despawn(&mut zombie), "20 blocks from a guest");
    assert_eq!(zombie.no_action_time, 0, "within 32 blocks of a guest: inaction reset");

    p.s.lan.as_mut().expect("LAN open").guests[0].gone = true;
    p.s.game_mode = 3;
    zombie.no_action_time = 900;
    assert!(!p.s.check_despawn(&mut zombie), "a spectator alone despawns nothing");
    assert_eq!(zombie.no_action_time, 900, "no player: inaction counter left alone");
}

// ---------- Spawn eggs ----------

/// The egg's name, its custom data and the fields of its entity data reach the mob once its
/// spawn is finalised; the mob keeps its own UUID, and its placement is announced from the
/// player.
#[test]
fn spawn_egg_hands_its_name_and_entity_data_to_the_mob() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-data") else { return };
    let data = compound(vec![
        ("id", Nbt::String("minecraft:zombie".into())),
        ("NoAI", Nbt::Byte(1)),
        ("Health", Nbt::Float(5.0)),
        ("UUID", nbtio::uuid_nbt(7)),
    ]);
    let custom_data = compound(vec![("quest", Nbt::Int(3))]);
    let named = egg(
        "minecraft:zombie_spawn_egg",
        vec![
            ("minecraft:custom_name", Nbt::String("Bob".into())),
            ("minecraft:custom_data", custom_data.clone()),
            ("minecraft:entity_data", data),
        ],
    );
    let before = mob_uuids(&p);
    p.s.mobs.rec.vib.log.clear();
    assert!(p.s.use_spawn_egg(&named, FLOOR, 1));
    let spawned = new_mobs(&p, &before);
    let zombie = spawned.iter().find(|m| m.kind == Kind::Zombie).expect("a zombie");
    assert_eq!(zombie.custom_name, Some(Nbt::String("Bob".into())), "name of the stack");
    assert_eq!(custom_data_of(zombie), Some(&custom_data), "custom data of the stack");
    assert!(zombie.no_ai, "entity data read");
    assert_eq!(zombie.health, 5.0, "entity data read over the finalised mob");
    assert_ne!(zombie.uuid, 7, "the mob keeps its UUID");
    assert_eq!(zombie.block_pos(), ABOVE_FLOOR);
    let centre_above_floor = ABOVE_FLOOR.map(|v| f64::from(v) + 0.5);
    assert_eq!(entity_place_events(&p), [(centre_above_floor, Some(Src::Living(Ent::Player)))], "placement event");
    // A plain egg changes nothing.
    let before = mob_uuids(&p);
    assert!(p.s.use_spawn_egg(&egg("minecraft:zombie_spawn_egg", Vec::new()), [220, -60, 216], 1));
    let spawned = new_mobs(&p, &before);
    let plain = spawned.iter().find(|m| m.kind == Kind::Zombie).expect("a zombie");
    assert!(plain.custom_name.is_none() && !plain.no_ai && plain.health == 20.0);
    assert_eq!(custom_data_of(plain), None);
}

/// From a dispenser and in a liquid, the egg hands its stack to the mob like on a block.
#[test]
fn spawn_egg_data_reaches_the_mob_from_a_dispenser_and_in_a_liquid() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-paths") else { return };
    let named_egg = || {
        let data = compound(vec![("id", Nbt::String("minecraft:zombie".into())), ("NoAI", Nbt::Byte(1))]);
        egg(
            "minecraft:zombie_spawn_egg",
            vec![("minecraft:custom_name", Nbt::String("Bob".into())), ("minecraft:entity_data", data)],
        )
    };
    let the_new_zombie = |p: &Probe, before: &[u128]| -> ([i32; 3], Option<Nbt>, bool) {
        let spawned = new_mobs(p, before);
        let zombie = spawned.iter().find(|m| m.kind == Kind::Zombie).expect("a zombie");
        (zombie.block_pos(), zombie.custom_name.clone(), zombie.no_ai)
    };
    let bob = Some(Nbt::String("Bob".into()));
    // A dispenser facing up: the mob appears in the block above it.
    let before = mob_uuids(&p);
    assert!(p.s.dispense_item(SPAWNER, Dir::Up, 0, named_egg()).is_none(), "the egg leaves the dispenser");
    assert_eq!(the_new_zombie(&p, &before), ([216, -58, 212], bob.clone(), true), "from a dispenser");
    // A player aiming down at a water source: the mob appears in the source block.
    cmd(&mut p, "setblock 220 -60 210 minecraft:water");
    p.s.pos = [220.5, -59.0, 210.5];
    p.s.pitch = 90.0;
    let hand = 36 + p.s.held as usize;
    p.s.slots[hand] = Some(named_egg());
    let before = mob_uuids(&p);
    assert!(p.s.ray_item_use(0));
    assert_eq!(the_new_zombie(&p, &before), ([220, -60, 210], bob, true), "in a liquid");
    assert!(p.s.slots[hand].is_none(), "the egg is used up");
}

/// A spawn egg used on a mob of its species: the baby takes the name and the custom data of
/// the stack.
#[test]
fn baby_born_from_a_spawn_egg_takes_the_name_and_custom_data_of_the_stack() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-baby") else { return };
    let custom_data = compound(vec![("quest", Nbt::Int(3))]);
    let cow = p.s.new_mob(Kind::Cow, [216.5, -59.0, 216.5], 0.0);
    let cow_id = cow.id;
    p.s.add_mob(cow);
    let hand = 36 + p.s.held as usize;
    p.s.slots[hand] = Some(egg(
        "minecraft:cow_spawn_egg",
        vec![("minecraft:custom_name", Nbt::String("Bob".into())), ("minecraft:custom_data", custom_data.clone())],
    ));
    let before = mob_uuids(&p);
    // An interaction works on a mob taken out of the list.
    let slot = p.s.mob_index(cow_id).expect("the cow");
    let mut parent = p.s.mobs.list.take(slot).expect("the cow");
    assert!(p.s.important_interact(&mut parent, hand), "the egg is used on the cow");
    p.s.mobs.list.put(slot, parent);
    let born = new_mobs(&p, &before);
    assert_eq!(born.len(), 1);
    assert!(born[0].is_baby());
    assert_eq!(born[0].custom_name, Some(Nbt::String("Bob".into())), "name of the stack");
    assert_eq!(custom_data_of(born[0]), Some(&custom_data), "custom data of the stack");
}

fn text(value: &str) -> Nbt {
    Nbt::String(value.into())
}

fn registry_index(p: &Probe, registry: &str, id: &str) -> i32 {
    let entries = p.s.regs.entries(registry);
    entries.iter().position(|entry| entry == id).unwrap_or_else(|| panic!("{id} not in {registry}")) as i32
}

/// The mob a spawn egg with these components places on the platform floor at `x`.
fn mob_from_egg(p: &mut Probe, item: &str, components: Vec<(&str, Nbt)>, x: i32) -> Mob {
    let before = mob_uuids(p);
    assert!(p.s.use_spawn_egg(&egg(item, components), [x, -60, 216], 1), "{item} refused");
    let born = new_mobs(p, &before);
    assert_eq!(born.len(), 1, "{item}: one mob");
    born[0].clone()
}

/// The per-species components of an egg reach its mob once the spawn is finalised: variants,
/// sound variant, collar, colours, the horse's colour (its markings kept), the fish's look, the
/// killer bunny, the villager type, the absorbed block; a component of another species is
/// ignored.
#[test]
fn spawn_egg_hands_its_species_components_to_the_mob() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-components") else { return };
    let wolf = mob_from_egg(
        &mut p,
        "minecraft:wolf_spawn_egg",
        vec![
            ("minecraft:wolf/variant", text("minecraft:snowy")),
            ("minecraft:wolf/sound_variant", text("big")),
            ("minecraft:wolf/collar", text("lime")),
            ("minecraft:sheep/color", text("black")),
        ],
        202,
    );
    assert_eq!(wolf.variant, registry_index(&p, "minecraft:wolf_variant", "minecraft:snowy"), "wolf variant");
    assert_eq!(wolf.sound_variant, registry_index(&p, "minecraft:wolf_sound_variant", "minecraft:big"), "sound");
    assert_eq!(wolf.collar, 5, "lime collar");
    assert_eq!(wolf.wool, 0, "a sheep's colour is not a wolf's");

    let sheep = mob_from_egg(&mut p, "minecraft:sheep_spawn_egg", vec![("minecraft:sheep/color", text("black"))], 204);
    assert_eq!(sheep.wool, 15, "black wool");

    let horse =
        mob_from_egg(&mut p, "minecraft:horse_spawn_egg", vec![("minecraft:horse/variant", text("black"))], 206);
    assert_eq!(horse.variant & 0xFF, 4, "black horse");

    let fish = mob_from_egg(
        &mut p,
        "minecraft:tropical_fish_spawn_egg",
        vec![
            ("minecraft:tropical_fish/pattern", text("betty")),
            ("minecraft:tropical_fish/base_color", text("red")),
            ("minecraft:tropical_fish/pattern_color", text("blue")),
        ],
        208,
    );
    assert_eq!(fish.variant, super::sealife::fish::pack(1025, 14, 11), "betty, red on blue");

    let rabbit =
        mob_from_egg(&mut p, "minecraft:rabbit_spawn_egg", vec![("minecraft:rabbit/variant", text("evil"))], 210);
    assert_eq!(rabbit.variant, 99, "killer bunny");
    let killer_name = compound(vec![("translate", text("entity.minecraft.killer_bunny"))]);
    assert_eq!(rabbit.custom_name, Some(killer_name), "killer bunny name");

    let villager = mob_from_egg(
        &mut p,
        "minecraft:villager_spawn_egg",
        vec![("minecraft:villager/variant", text("minecraft:snow"))],
        212,
    );
    assert_eq!(villager.villager[0], 4, "snow villager");

    let frog =
        mob_from_egg(&mut p, "minecraft:frog_spawn_egg", vec![("minecraft:frog/variant", text("minecraft:cold"))], 214);
    assert_eq!(frog.variant, registry_index(&p, "minecraft:frog_variant", "minecraft:cold"), "cold frog");
    assert_eq!(frog.bm.frog_variant.as_deref(), Some("minecraft:cold"));

    let content = compound(vec![("id", text("minecraft:tnt"))]);
    let cube =
        mob_from_egg(&mut p, "minecraft:sulfur_cube_spawn_egg", vec![("minecraft:sulfur_cube_content", content)], 218);
    assert_eq!(cube.equipment[6].as_ref().map(|s| s.id.as_str()), Some("minecraft:tnt"), "absorbed block");
    assert_eq!(cube.drop_chance[6], 2.0, "dropped for sure");

    let llama =
        mob_from_egg(&mut p, "minecraft:trader_llama_spawn_egg", vec![("minecraft:llama/variant", text("gray"))], 220);
    assert_eq!(llama.variant, 3, "gray trader llama");
    let salmon =
        mob_from_egg(&mut p, "minecraft:salmon_spawn_egg", vec![("minecraft:salmon/size", text("large"))], 222);
    assert_eq!(salmon.variant, 2, "large salmon");
    let shulker =
        mob_from_egg(&mut p, "minecraft:shulker_spawn_egg", vec![("minecraft:shulker/color", text("red"))], 224);
    assert_eq!(shulker.end.color, 14, "red shulker");
}

/// A spawn egg used on a mob of its species: the baby takes the egg's species components too.
#[test]
fn baby_born_from_a_spawn_egg_takes_its_species_components() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-baby-components") else { return };
    let sheep = p.s.new_mob(Kind::Sheep, [216.5, -59.0, 216.5], 0.0);
    let sheep_id = sheep.id;
    p.s.add_mob(sheep);
    let hand = 36 + p.s.held as usize;
    p.s.slots[hand] = Some(egg("minecraft:sheep_spawn_egg", vec![("minecraft:sheep/color", text("purple"))]));
    let before = mob_uuids(&p);
    let slot = p.s.mob_index(sheep_id).expect("the sheep");
    let mut parent = p.s.mobs.list.take(slot).expect("the sheep");
    assert!(p.s.important_interact(&mut parent, hand), "the egg is used on the sheep");
    p.s.mobs.list.put(slot, parent);
    let born = new_mobs(&p, &before);
    assert_eq!(born.len(), 1);
    assert_eq!(born[0].wool, 10, "purple lamb");
}

/// An egg whose entity data names an entity that is not a mob places that entity, with the
/// stack's name and its entity data; a type that cannot be placed (a player) keeps the egg.
#[test]
fn spawn_egg_naming_an_entity_that_is_not_a_mob_places_it() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-armor-stand") else { return };
    let stand_data = compound(vec![("id", text("minecraft:armor_stand")), ("Small", Nbt::Byte(1))]);
    let stand_egg = egg(
        "minecraft:pig_spawn_egg",
        vec![("minecraft:entity_data", stand_data), ("minecraft:custom_name", text("Bob"))],
    );
    let mobs_before = p.s.mobs.list.iter().count();
    p.s.mobs.rec.vib.log.clear();
    assert!(p.s.use_spawn_egg(&stand_egg, FLOOR, 1), "armor stand placed");
    assert_eq!(p.s.mobs.list.iter().count(), mobs_before, "no mob");
    let stands: Vec<&Nbt> =
        p.s.entities
            .values()
            .flatten()
            .map(|(_, entity)| entity)
            .filter(|entity| entity.get("id").and_then(Nbt::as_str) == Some("minecraft:armor_stand"))
            .collect();
    assert_eq!(stands.len(), 1, "one armor stand");
    assert_eq!(stands[0].get("CustomName"), Some(&text("Bob")), "name of the stack");
    assert_eq!(nbtio::int(stands[0], "Small"), Some(1), "entity data of the stack");
    let pos = nbtio::f64s::<3>(stands[0].get("Pos")).expect("position");
    assert_eq!(pos, [216.5, -59.0, 216.5], "on the floor");
    assert_eq!(entity_place_events(&p).len(), 1, "placement event");

    let player_egg = egg(
        "minecraft:pig_spawn_egg",
        vec![("minecraft:entity_data", compound(vec![("id", text("minecraft:player"))]))],
    );
    assert!(!p.s.use_spawn_egg(&player_egg, [220, -60, 216], 1), "a player cannot be placed");
}

/// The species is the one of the egg's entity data, whatever the item.
#[test]
fn spawn_egg_species_comes_from_its_entity_data() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-species") else { return };
    let cow_in_pig_egg = egg(
        "minecraft:pig_spawn_egg",
        vec![("minecraft:entity_data", compound(vec![("id", Nbt::String("minecraft:cow".into()))]))],
    );
    let before = mob_uuids(&p);
    assert!(p.s.use_spawn_egg(&cow_in_pig_egg, FLOOR, 1));
    let kinds: Vec<Kind> = new_mobs(&p, &before).iter().map(|m| m.kind).collect();
    assert_eq!(kinds, [Kind::Cow]);
}

/// Egg on a monster spawner: an operator's egg hands its whole entity data to the spawner,
/// another player's only its species.
#[test]
fn only_an_operator_egg_hands_its_entity_data_to_a_spawner() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-spawner") else { return };
    let data = compound(vec![("id", Nbt::String("minecraft:skeleton".into())), ("NoAI", Nbt::Byte(1))]);
    let skeleton_egg = egg("minecraft:skeleton_spawn_egg", vec![("minecraft:entity_data", data)]);
    let spawner_entity = |p: &Probe, at: [i32; 3]| {
        p.s.be_get(at).and_then(|be| be.get("SpawnData")).and_then(|data| data.get("entity")).cloned()
    };
    let other_spawner = [220, -59, 212];
    cmd(&mut p, "setblock 216 -59 212 minecraft:spawner");
    cmd(&mut p, "setblock 220 -59 212 minecraft:spawner");
    p.s.cmd.op = 4;
    assert!(p.s.use_spawn_egg(&skeleton_egg, SPAWNER, 1));
    let by_operator = spawner_entity(&p, SPAWNER).expect("entity set");
    assert_eq!(by_operator.get("id"), Some(&Nbt::String("minecraft:skeleton".into())));
    assert_eq!(by_operator.get("NoAI"), Some(&Nbt::Byte(1)), "operator: entity data handed over");
    p.s.cmd.op = 0;
    assert!(p.s.use_spawn_egg(&skeleton_egg, other_spawner, 1));
    let by_other = spawner_entity(&p, other_spawner).expect("entity set");
    assert_eq!(by_other, compound(vec![("id", Nbt::String("minecraft:skeleton".into()))]), "species only");
}

/// Egg on a trial spawner: the same rule, for the single mob both its configurations then spawn.
#[test]
fn only_an_operator_egg_hands_its_entity_data_to_a_trial_spawner() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-trial-spawner") else { return };
    let data = compound(vec![("id", Nbt::String("minecraft:skeleton".into())), ("NoAI", Nbt::Byte(1))]);
    let skeleton_egg = egg("minecraft:skeleton_spawn_egg", vec![("minecraft:entity_data", data.clone())]);
    let species_only = compound(vec![("id", Nbt::String("minecraft:skeleton".into()))]);
    let spawned_entities = |p: &Probe, at: [i32; 3]| {
        ["normal_config", "ominous_config"].map(|configuration| {
            let potentials =
                p.s.be_get(at).and_then(|be| be.get(configuration)).and_then(|c| c.get("spawn_potentials"));
            let [only] = nbtio::list(potentials) else { panic!("one mob in {configuration}: {potentials:?}") };
            only.get("data").and_then(|data| data.get("entity")).cloned().expect("an entity")
        })
    };
    let other_spawner = [220, -59, 212];
    cmd(&mut p, "setblock 216 -59 212 minecraft:trial_spawner");
    cmd(&mut p, "setblock 220 -59 212 minecraft:trial_spawner");
    p.s.cmd.op = 4;
    assert!(p.s.use_spawn_egg(&skeleton_egg, SPAWNER, 1));
    assert_eq!(spawned_entities(&p, SPAWNER), [data.clone(), data], "operator: entity data handed over");
    p.s.cmd.op = 0;
    assert!(p.s.use_spawn_egg(&skeleton_egg, other_spawner, 1));
    assert_eq!(spawned_entities(&p, other_spawner), [species_only.clone(), species_only], "species only");
}

// ---------- Monster spawner ----------

/// A monster spawner spawns an entity that is not a mob (primed TNT, read from its compound),
/// up to its cap on nearby entities of that type.
#[test]
fn monster_spawner_spawns_a_non_mob_entity() {
    let Some((mut p, _dir)) = open("spawn-rules-spawner-tnt") else { return };
    p.run(ON_PLATFORM);
    cmd(
        &mut p,
        "setblock 216 -59 212 minecraft:spawner{Delay:0s,SpawnCount:40s,MaxNearbyEntities:6s,\
         SpawnData:{entity:{id:\"minecraft:tnt\",fuse:40s}}}",
    );
    let players = p.s.alive_players();
    p.s.spawner_tick(SPAWNER, &players);
    let fuses: Vec<i32> = p
        .s
        .bs
        .entities
        .list
        .iter()
        .filter(|e| !e.removed)
        .filter_map(|e| match e.kind {
            EntKind::Tnt(fuse) => Some(fuse),
            EntKind::Falling(_) => None,
        })
        .collect();
    assert_eq!(fuses, [40; 6], "six primed TNT with the fuse of the spawn data, then the cap");
    let delay = p.s.be_get(SPAWNER).and_then(|be| nbtio::int(be, "Delay")).unwrap_or(0);
    assert!(delay >= 200, "a new delay is drawn: {delay}");
    // An unknown type spawns nothing and draws a new delay too.
    cmd(&mut p, "setblock 220 -59 212 minecraft:spawner{Delay:0s,SpawnData:{entity:{id:\"minecraft:nothing\"}}}");
    p.s.spawner_tick([220, -59, 212], &players);
    let delay = p.s.be_get([220, -59, 212]).and_then(|be| nbtio::int(be, "Delay")).unwrap_or(0);
    assert!(delay >= 200, "unknown type: new delay: {delay}");
}

/// The cap on nearby entities counts the projectiles in flight: a spawner of arrows stops at it.
#[test]
fn spawner_cap_counts_projectiles_in_flight() {
    let Some((mut p, _dir)) = open("spawn-rules-spawner-arrows") else { return };
    p.run(ON_PLATFORM);
    cmd(
        &mut p,
        "setblock 216 -59 212 minecraft:spawner{Delay:0s,SpawnCount:12s,MaxNearbyEntities:3s,\
         SpawnData:{entity:{id:\"minecraft:arrow\"}}}",
    );
    let players = p.s.alive_players();
    p.s.spawner_tick(SPAWNER, &players);
    let arrows = p.s.proj.list.iter().filter(|arrow| !arrow.removed && arrow.kind.name() == "minecraft:arrow").count();
    assert_eq!(arrows, 3, "three arrows, then the cap");
}

/// The cap on nearby entities counts the entities of the class of the spawned one: boats of any
/// wood stop a boat spawner, and do not count for a chest boat spawner.
#[test]
fn spawner_cap_counts_boats_of_any_wood_together() {
    let Some((mut p, _dir)) = open("spawn-rules-spawner-boats") else { return };
    p.run(ON_PLATFORM);
    for x in 213..219 {
        cmd(&mut p, &format!("summon minecraft:birch_boat {x}.5 -59 214.5"));
    }
    let entities_of_type = |p: &Probe, type_name: &str| {
        p.s.all_ents().into_iter().filter(|&entity| p.s.ent_type(entity) == type_name).count()
    };
    assert_eq!(entities_of_type(&p, "minecraft:birch_boat"), 6);
    let players = p.s.alive_players();
    // Both spawners have the six birch boats in their range.
    let tick_a_spawner_of = |p: &mut Probe, type_name: &str, at: [i32; 3]| {
        let spawner = format!(
            "setblock {} {} {} minecraft:spawner{{Delay:0s,SpawnCount:40s,MaxNearbyEntities:6s,\
             SpawnData:{{entity:{{id:\"{type_name}\"}}}}}}",
            at[0], at[1], at[2]
        );
        cmd(p, &spawner);
        p.s.spawner_tick(at, &players);
        p.s.be_get(at).and_then(|be| nbtio::int(be, "Delay")).unwrap_or(0)
    };
    let delay = tick_a_spawner_of(&mut p, "minecraft:oak_boat", SPAWNER);
    assert_eq!(entities_of_type(&p, "minecraft:oak_boat"), 0, "six birch boats fill the cap of an oak boat spawner");
    assert!(delay >= 200, "a new delay is drawn: {delay}");
    tick_a_spawner_of(&mut p, "minecraft:oak_chest_boat", [216, -59, 216]);
    assert_eq!(entities_of_type(&p, "minecraft:oak_chest_boat"), 6, "chest boats are counted apart, up to the cap");
}

// ---------- What a mob reads of its surroundings ----------

/// The light value of mobs is raised by the ambient light of the dimension type: 0.25 in the
/// End, 0.1 in the Nether, nothing in the overworld.
#[test]
fn light_value_is_raised_by_the_ambient_light_of_the_dimension() {
    use super::env::light_curve_with_ambient;
    assert_eq!(light_curve_with_ambient(0, 0.0), 0.0);
    assert_eq!(light_curve_with_ambient(0, 0.1), 0.1);
    assert_eq!(light_curve_with_ambient(0, 0.25), 0.25);
    assert_eq!(light_curve_with_ambient(15, 0.25), 1.0);
    // Brightness 7: curve 7/15 / (4 - 3 × 7/15), then blended towards 1.
    let curve = (7.0f32 / 15.0) / (4.0 - 3.0 * (7.0f32 / 15.0));
    assert_eq!(light_curve_with_ambient(7, 0.0), curve);
    assert_eq!(light_curve_with_ambient(7, 0.25), curve + 0.25 * (1.0 - curve));
    let Some(data) = rc_testdata::version_data() else { return };
    let ambient = |name: &str| crate::world::DimKind::of(&data, name).ambient_light;
    assert_eq!(
        (ambient("minecraft:overworld"), ambient("minecraft:the_nether"), ambient("minecraft:the_end")),
        (0.0, 0.1, 0.25)
    );
    assert_eq!(ambient("test:absent"), 0.0);
}

/// In the box scanned by `biome_of_a_block_goes_through_the_zoom` (test world seed): blocks near
/// a biome border whose biome, read through the zoom, is not the one of their own 4×4×4 cell.
const BLOCKS_MOVED_BY_THE_ZOOM: usize = 556;
/// In the same box, blocks whose eight candidate cells lie in one section holding a single
/// biome, outside the filled column: the least the shortcut reads without the zoom.
const BLOCKS_WHOSE_CELLS_SHARE_A_UNIFORM_SECTION: usize = 4320;

/// The biome of a block is the one of the cell the fuzzy zoom picks, which near a border differs
/// from the block's own cell; an environment attribute still reads the block's own cell. The
/// shortcut of the uniform sections gives the same biome as the zoom, block by block.
#[test]
fn biome_of_a_block_goes_through_the_zoom() {
    let Some((mut p, _dir)) = open("spawn-rules-biome-zoom") else { return };
    // Two borders: around a whole chunk column (uniform sections on both sides), and inside a
    // section (a mixed one, where the shortcut must not apply).
    cmd(&mut p, "fillbiome 208 -64 208 223 -40 223 minecraft:mushroom_fields");
    cmd(&mut p, "fillbiome 200 -64 200 203 -57 207 minecraft:desert");
    let mushroom_fields = p.s.stored_biome_at([216, -60, 216]).expect("loaded");
    assert_eq!(p.s.stored_biome_str([216, -60, 216]), "minecraft:mushroom_fields");
    let desert = biome_index(&p, "minecraft:desert") as u32;
    // The bottom section of the filled column as a chunk read from disk holds it: one biome for
    // the whole section, next to the uniform sections of the neighbouring columns.
    let Some(Some(filled)) = p.s.chunks.get_mut(&(13, 13)) else { panic!("chunk of the platform") };
    filled.sections[0].biomes = rc_world::Container::Single(mushroom_fields as u32);
    // Three more uniform sections of that column, each of a biome its neighbours in height and
    // around do not hold: the second one (a border between two sections in height) and the two
    // at the top of the world.
    let top_section = filled.sections.len() - 1;
    filled.sections[1].biomes = rc_world::Container::Single(desert);
    filled.sections[top_section - 1].biomes = rc_world::Container::Single(desert);
    filled.sections[top_section].biomes = rc_world::Container::Single(mushroom_fields as u32);
    let seed = p.s.wgs.zoom_seed;
    let (mut moved, mut without_zoom, mut sabotage_caught) = (0, 0, 0);
    for x in 196..232 {
        for z in 196..232 {
            for y in -62..-54 {
                let block = [x, y, z];
                let (qx, qy, qz) = rc_worldgen::biome::zoom(seed, x, y, z);
                let through_zoom = p.s.stored_biome_at([qx << 2, qy << 2, qz << 2]);
                assert_eq!(p.s.biome_at_with(block, false), through_zoom, "always zoomed, {block:?}");
                assert_eq!(p.s.biome_at_with(block, true), through_zoom, "with the shortcut, {block:?}");
                let in_tag = p.s.biome_tag(block, "minecraft:without_zombie_sieges");
                assert_eq!(in_tag, through_zoom == Some(mushroom_fields), "tag read on the same biome at {block:?}");
                moved += usize::from(through_zoom != p.s.stored_biome_at(block));
                without_zoom += usize::from(p.s.biome_of_uniform_sections_around(block).is_some());
                let sabotaged = p.s.biome_of_uniform_sections_sabotaged(block);
                sabotage_caught += usize::from(sabotaged.is_some() && sabotaged != through_zoom);
            }
        }
    }
    assert!(sabotage_caught > 0, "the box catches a shortcut that does not compare its sections");
    assert_eq!(moved, BLOCKS_MOVED_BY_THE_ZOOM, "blocks whose biome is not their own cell's");
    assert!(
        without_zoom >= BLOCKS_WHOSE_CELLS_SHARE_A_UNIFORM_SECTION,
        "blocks read without the zoom: {without_zoom}"
    );
    // Across two sections in height (y = -48 is the first layer of the second one), around and
    // inside the filled column, whose two sections differ.
    assert_biome_reads_match_the_zoom(&p, 204..228, -52..=-44);
    // At the floor and at the ceiling of the world one of the candidate cells lies outside it:
    // the shortcut still serves those blocks, from the nearest section like the stored read.
    let min_y = p.s.dim.height.min_y;
    let max_y = min_y + p.s.dim.height.sections as i32 * 16 - 1;
    let served_at_the_floor = assert_biome_reads_match_the_zoom(&p, 204..228, min_y..=min_y + 1);
    assert!(served_at_the_floor > 0, "no block of the floor read without the zoom");
    let served_at_the_ceiling = assert_biome_reads_match_the_zoom(&p, 204..228, max_y..=max_y);
    assert!(served_at_the_ceiling > 0, "no block of the ceiling read without the zoom");
    // The border between the two top sections.
    assert_biome_reads_match_the_zoom(&p, 204..228, max_y - 18..=max_y - 12);
}

/// Environment attributes other than the mob spawns are read on the block's own stored cell:
/// at a block the zoom moves across a biome border they follow the cell, not the zoomed biome.
#[test]
fn environment_attributes_follow_the_stored_cell_of_the_block() {
    let Some((mut p, _dir)) = open("spawn-rules-attribute-cell") else { return };
    // No pillager patrol and a faster fire burnout in mushroom fields, snow golems melt in a
    // desert, fewer animals come with a new chunk in snowy plains.
    cmd(&mut p, "fillbiome 208 -64 208 223 -40 223 minecraft:mushroom_fields");
    cmd(&mut p, "fillbiome 200 -64 200 203 -57 207 minecraft:desert");
    cmd(&mut p, "fillbiome 224 -64 196 231 -57 203 minecraft:snowy_plains");
    // Patrols only start after 120,000 ticks of the overworld clock.
    p.s.time = 130_000;
    for clock in p.s.clocks.iter_mut().filter(|clock| clock.0 == "minecraft:overworld") {
        clock.1 = 130_000;
    }
    let (in_mushroom_fields, beside_them) =
        blocks_moved_across_the_border_of(&p, biome_index(&p, "minecraft:mushroom_fields"));
    assert!(!p.s.can_patrol_spawn(in_mushroom_fields), "patrol in a cell of mushroom fields");
    assert!(p.s.can_patrol_spawn(beside_them), "no patrol beside the mushroom fields");
    assert!(p.s.increased_fire_burnout(in_mushroom_fields), "fire in a cell of mushroom fields");
    assert!(!p.s.increased_fire_burnout(beside_them), "fire beside the mushroom fields");
    // The boolean read shared by the piglins, the raids and the portals.
    let patrols = "minecraft:gameplay/can_pillager_patrol_spawn";
    assert!(!super::piglins::env_bool(&p.s, in_mushroom_fields, patrols, true), "shared boolean read, in the cell");
    assert!(super::piglins::env_bool(&p.s, beside_them, patrols, true), "shared boolean read, beside it");
    let (in_desert, beside_it) = blocks_moved_across_the_border_of(&p, biome_index(&p, "minecraft:desert"));
    assert!(p.s.snow_golem_melts(in_desert), "snow golem in a cell of desert");
    assert!(!p.s.snow_golem_melts(beside_it), "snow golem beside the desert");
    let (in_snowy_plains, beside_them) =
        blocks_moved_across_the_border_of(&p, biome_index(&p, "minecraft:snowy_plains"));
    assert_eq!(p.s.creature_generation_spawn_probability(in_snowy_plains), 0.07);
    assert_eq!(p.s.creature_generation_spawn_probability(beside_them), 0.1);
}

/// Animals placed with a new chunk: the biome of its corner block is the one of the cell the
/// zoom picks; when that cell belongs to a neighbour chunk that has not arrived yet, the world
/// generator gives its biome instead of the block's own cell.
#[test]
fn generation_spawns_ask_the_generator_for_the_biome_of_a_chunk_not_arrived() {
    let Some((mut p, _dir)) = open("spawn-rules-generation-biome") else { return };
    let min_y = p.s.dim.height.min_y;
    let max_y = min_y + p.s.dim.height.sections as i32 * 16 - 1;
    let seed = p.s.wgs.zoom_seed;
    // A chunk around the platform whose corner block reads a cell of a loaded neighbour chunk.
    let chunk_of = |block: [i32; 3]| (block[0] >> 4, block[2] >> 4);
    let (corner, chosen_cell) = (11..=15)
        .flat_map(|chunk_x| (11..=15).map(move |chunk_z| [chunk_x * 16, max_y, chunk_z * 16]))
        .find_map(|corner| {
            let (qx, qy, qz) = rc_worldgen::biome::zoom(seed, corner[0], corner[1], corner[2]);
            let cell = [qx << 2, qy << 2, qz << 2];
            let in_a_loaded_neighbour = chunk_of(cell) != chunk_of(corner) && p.s.stored_biome_at(cell).is_some();
            in_a_loaded_neighbour.then_some((corner, cell))
        })
        .expect("a corner block read in a neighbour chunk");
    // Its own chunk holds mushroom fields, the neighbour a desert.
    let (mushroom_fields, desert) = (biome_index(&p, "minecraft:mushroom_fields"), biome_index(&p, "minecraft:desert"));
    for (position, biome) in [(chunk_of(corner), mushroom_fields), (chunk_of(chosen_cell), desert)] {
        let Some(Some(chunk)) = p.s.chunks.get_mut(&position) else { panic!("chunk {position:?} loaded") };
        for section in &mut chunk.sections {
            section.biomes = rc_world::Container::Single(biome as u32);
        }
    }
    assert_eq!(p.s.biome_for_generation_spawns(corner), Some(desert), "neighbour there: its stored cell");
    // The neighbour has not arrived: the generator's biome for that cell, inside the world.
    p.s.chunks.remove(&chunk_of(chosen_cell));
    let cell_in_world = [chosen_cell[0], chosen_cell[1].clamp(min_y, max_y), chosen_cell[2]];
    let generated = p.s.generated_biomes(&[cell_in_world])[0].expect("the generator of the test world");
    assert!(generated != mushroom_fields && generated != desert, "the test world generates another biome");
    assert_eq!(p.s.biome_for_generation_spawns(corner), Some(generated), "neighbour not arrived: the generator");
}

// ---------- Explosions ----------

/// The knockback of an explosion is scaled by the explosion knockback resistance of the mob.
#[test]
fn explosion_knockback_is_scaled_by_the_resistance_attribute() {
    let Some((mut p, _dir)) = open("spawn-rules-blast-resistance") else { return };
    let mut add_zombie = |at: [f64; 3], resistance: f64| {
        let mut zombie = p.s.new_mob(Kind::Zombie, at, 0.0);
        zombie.attrs.explosion_kb_resistance.base = resistance;
        let uuid = zombie.uuid;
        p.s.add_mob(zombie);
        uuid
    };
    let pushed = add_zombie([210.5, -59.0, 216.5], 0.0);
    let half = add_zombie([222.5, -59.0, 216.5], 0.5);
    let anchored = add_zombie([216.5, -59.0, 222.5], 1.0);
    p.s.explode_core(None, None, None, [216.5, -59.0, 216.5], 4.0, false, "minecraft:explosion");
    let velocity = |uuid: u128| p.s.mobs.list.iter().find(|m| m.uuid == uuid).expect("still there").vel;
    assert!(velocity(pushed)[0] < 0.0, "pushed west: {:?}", velocity(pushed));
    assert_eq!(velocity(anchored), [0.0; 3], "full resistance");
    let ratio = velocity(half)[0] / -velocity(pushed)[0];
    assert!((ratio - 0.5).abs() < 1e-9, "half resistance, half the push: {ratio}");
}

// ---------- Measurement ----------

/// Cost of the natural spawn pass at midnight: while the world fills up to the caps (spawn
/// attempts on every pass), then at the caps (census and chunk walk only).
#[test]
#[ignore = "measurement bench"]
fn bench_natural_spawn_pass() {
    let Some((mut p, _dir)) = open("bench-natural-spawn-pass") else { return };
    p.run(ON_PLATFORM);
    night_with_natural_spawns(&mut p);
    let pass = |p: &mut Probe, count: u32| {
        let start = Instant::now();
        spawn_passes(p, count, false);
        start.elapsed().as_secs_f64() * 1e6 / f64::from(count)
    };
    let filling = pass(&mut p, 300);
    let mobs_after_filling = p.s.mobs.list.iter().count();
    let _settle = pass(&mut p, 3000);
    let mut at_caps: Vec<f64> = (0..5).map(|_| pass(&mut p, 2000)).collect();
    at_caps.sort_by(f64::total_cmp);
    println!(
        "natural spawn pass: filling {filling:.1} us/pass ({mobs_after_filling} mobs after 300 passes), \
         at the caps {:.2} us/pass (median of 5 x 2000, {} mobs)",
        at_caps[2],
        p.s.mobs.list.iter().count()
    );
}

/// Cost of reading the biome of a block: its own stored cell, through the zoom, with the zoom
/// skipped in uniform sections, and both with the world generator's memoised zoom; on scattered
/// blocks and along the walk of spawn groups.
#[test]
#[ignore = "measurement bench"]
fn bench_biome_of_a_block() {
    let Some((p, _dir)) = open("bench-biome-of-a-block") else { return };
    let seed = p.s.wgs.zoom_seed;
    let scattered: Vec<[i32; 3]> =
        (0..4096).map(|i| [200 + (i * 7) % 32, -60 + (i * 3) % 8, 200 + (i * 13) % 32]).collect();
    // Groups like the natural spawns': twelve steps of at most five blocks each way at one height.
    let mut rng = super::jrand::JRand::new(1);
    let mut group_walk = Vec::new();
    while group_walk.len() < 4096 {
        let y = -60 + rng.next_int_bound(8);
        let (mut x, mut z) = (200 + rng.next_int_bound(32), 200 + rng.next_int_bound(32));
        for _ in 0..12 {
            x += rng.next_int_bound(6) - rng.next_int_bound(6);
            z += rng.next_int_bound(6) - rng.next_int_bound(6);
            group_walk.push([x, y, z]);
        }
    }
    let memoised_zoom = |b: [i32; 3]| {
        let (qx, qy, qz) = rc_worldgen::biome::zoom_cached(seed, b[0], b[1], b[2]);
        p.s.stored_biome_at([qx << 2, qy << 2, qz << 2])
    };
    for (order, blocks) in [("scattered", &scattered), ("group walk", &group_walk)] {
        let time = |read: &dyn Fn([i32; 3]) -> Option<usize>| {
            let mut best = f64::MAX;
            for _ in 0..7 {
                let start = Instant::now();
                let mut sum = 0;
                for _ in 0..50 {
                    for &b in blocks {
                        sum += read(std::hint::black_box(b)).unwrap_or(0);
                    }
                }
                std::hint::black_box(sum);
                best = best.min(start.elapsed().as_secs_f64() * 1e9 / (50.0 * blocks.len() as f64));
            }
            best
        };
        let stored = time(&|b| p.s.stored_biome_at(b));
        let zoomed = time(&|b| p.s.biome_at_with(b, false));
        let shortcut = time(&|b| p.s.biome_at_with(b, true));
        let memoised = time(&memoised_zoom);
        let both = time(&|b| p.s.biome_of_uniform_sections_around(b).or_else(|| memoised_zoom(b)));
        let skipped = blocks.iter().filter(|&&b| p.s.biome_of_uniform_sections_around(b).is_some()).count();
        println!(
            "biome of a block, {order} ({skipped} of {} skip the zoom), ns/read, best of 7: stored cell {stored:.1}, \
             always zoomed {zoomed:.1}, zoom skipped in uniform sections {shortcut:.1}, memoised zoom {memoised:.1}, \
             skipped + memoised {both:.1}",
            blocks.len()
        );
    }
}

/// Cost of the biome read made once per generated chunk for its animals, in a new world of the
/// default generator: the chosen cell stored in a loaded neighbour chunk, then asked to the world
/// generator (neighbour not arrived).
#[test]
#[ignore = "measurement bench"]
fn bench_generation_spawn_biome() {
    let Some(data) = rc_testdata::version_data().map(std::sync::Arc::new) else { return };
    let folder = format!("rc-bench-generation-spawn-biome-{}", std::process::id());
    let dir = rc_testdata::scratch(std::env::temp_dir().join(folder));
    let mut world = crate::newworld::NewWorld::new("bench");
    world.seed = crate::tests_opt::SEED;
    crate::newworld::create_world(&dir, &data, &world).expect("world creation");
    let mut s = crate::portal_tests::open_world(&dir, data);
    crate::tests_opt::go(&mut s, crate::dims::OVERWORLD, 8.5, None, 8.5, false);
    let max_y = s.dim.height.min_y + s.dim.height.sections as i32 * 16 - 1;
    let seed = s.wgs.zoom_seed;
    // Corners of the loaded chunks whose block reads a cell of a neighbour chunk, loaded or not
    // (beyond the rim of the loaded square).
    let (mut neighbour_loaded, mut neighbour_missing) = (Vec::new(), Vec::new());
    for chunk_x in -6..=6 {
        for chunk_z in -6..=6 {
            let corner = [chunk_x * 16, max_y, chunk_z * 16];
            let (qx, qy, qz) = rc_worldgen::biome::zoom(seed, corner[0], corner[1], corner[2]);
            if (qx >> 2, qz >> 2) == (chunk_x, chunk_z) {
                continue;
            }
            let loaded = s.stored_biome_at([qx << 2, qy << 2, qz << 2]).is_some();
            (if loaded { &mut neighbour_loaded } else { &mut neighbour_missing }).push(corner);
        }
    }
    let mut time = |corners: &[[i32; 3]]| {
        let mut best = f64::MAX;
        for _ in 0..7 {
            let start = Instant::now();
            let mut sum = 0;
            for _ in 0..200 {
                for &corner in corners {
                    sum += s.biome_for_generation_spawns(std::hint::black_box(corner)).unwrap_or(0);
                }
            }
            std::hint::black_box(sum);
            best = best.min(start.elapsed().as_secs_f64() * 1e9 / (200.0 * corners.len() as f64));
        }
        best
    };
    let from_a_loaded_neighbour = time(&neighbour_loaded);
    let from_the_generator = time(&neighbour_missing);
    println!(
        "biome of a new chunk's corner, ns/read, best of 7: stored in a loaded neighbour {from_a_loaded_neighbour:.1} \
         ({} corners), asked to the generator {from_the_generator:.1} ({} corners)",
        neighbour_loaded.len(),
        neighbour_missing.len()
    );
    s.stop_gen();
    s.stop_parked();
}
