//! Natural spawns, spawn eggs and spawner blocks checked against the rules of the game: where a
//! group may start and land (world spawn, world border, copper grates, per-player cap), what a
//! spawn egg hands to its mob, what a monster spawner can spawn, which biome and which ambient
//! light a mob reads.

use std::time::Instant;

use rc_proto::nbt::Nbt;

use super::mob::Mob;
use super::spawn::PlayerMobCaps;
use super::species::{Category, Kind};
use super::tests_spawn_paths::{add_guest, open};
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

// ---------- Spawn eggs ----------

/// The egg's name and the fields of its entity data reach the mob once its spawn is finalised;
/// the mob keeps its own UUID.
#[test]
fn spawn_egg_hands_its_name_and_entity_data_to_the_mob() {
    let Some((mut p, _dir)) = open("spawn-rules-egg-data") else { return };
    let data = compound(vec![
        ("id", Nbt::String("minecraft:zombie".into())),
        ("NoAI", Nbt::Byte(1)),
        ("Health", Nbt::Float(5.0)),
        ("UUID", nbtio::uuid_nbt(7)),
    ]);
    let named = egg(
        "minecraft:zombie_spawn_egg",
        vec![("minecraft:custom_name", Nbt::String("Bob".into())), ("minecraft:entity_data", data)],
    );
    let before = mob_uuids(&p);
    assert!(p.s.use_spawn_egg(&named, FLOOR, 1));
    let spawned = new_mobs(&p, &before);
    let zombie = spawned.iter().find(|m| m.kind == Kind::Zombie).expect("a zombie");
    assert_eq!(zombie.custom_name, Some(Nbt::String("Bob".into())), "name of the stack");
    assert!(zombie.no_ai, "entity data read");
    assert_eq!(zombie.health, 5.0, "entity data read over the finalised mob");
    assert_ne!(zombie.uuid, 7, "the mob keeps its UUID");
    assert_eq!(zombie.block_pos(), ABOVE_FLOOR);
    // A plain egg changes nothing.
    let before = mob_uuids(&p);
    assert!(p.s.use_spawn_egg(&egg("minecraft:zombie_spawn_egg", Vec::new()), [220, -60, 216], 1));
    let spawned = new_mobs(&p, &before);
    let plain = spawned.iter().find(|m| m.kind == Kind::Zombie).expect("a zombie");
    assert!(plain.custom_name.is_none() && !plain.no_ai && plain.health == 20.0);
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
    // The bottom section of the filled column as a chunk read from disk holds it: one biome for
    // the whole section, next to the uniform sections of the neighbouring columns.
    let Some(Some(filled)) = p.s.chunks.get_mut(&(13, 13)) else { panic!("chunk of the platform") };
    filled.sections[0].biomes = rc_world::Container::Single(mushroom_fields as u32);
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
