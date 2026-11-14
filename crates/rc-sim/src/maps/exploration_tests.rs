//! Exploration maps on a new world (generated, structures on): a cartographer's offers, the
//! structure the origin stands in designated first.

use rc_proto::nbt::Nbt;
use serde_json::json;

use super::ExplorationSettings;
use crate::items::component;
use crate::mobs::species::Kind;
use crate::mobs::variants::{PROFESSIONS, VILLAGER_TYPES};

/// Seed of the test world (a plains village and other structures within the maps' reach).
const SEED: i64 = 20_261_114;

fn index_of(list: &[&str], name: &str) -> i32 {
    list.iter().position(|n| *n == name).unwrap_or_else(|| panic!("{name} missing")) as i32
}

/// A desert cartographer reaching level 2 draws 2 offers among the glass pane and the four
/// trades its type may have (jungle temple, plains, savanna village maps): each map is made
/// during the draw (a number and a "+" target), so its offer counts. Before, every map offer was
/// discarded by its own filter and the cartographer offered the glass pane alone.
#[test]
fn cartographer_level_two_offers_a_map() {
    let Some((mut s, _dir)) = crate::portal_tests::open_new_world("cartographer-maps", SEED) else { return };
    let mut m = s.new_mob(Kind::Villager, [0.5, 80.0, 0.5], 0.0);
    m.villager = [index_of(&VILLAGER_TYPES, "desert"), index_of(&PROFESSIONS, "cartographer"), 2];
    let offers = s.npc_offers(&mut m);
    let sold: Vec<&str> = offers.iter().map(|o| o.result.id.as_str()).collect();
    eprintln!("cartographer level 2 sells {sold:?}");
    assert_eq!(offers.len(), 2, "two offers: {sold:?}");
    let maps: Vec<_> = offers.iter().filter(|o| crate::maps::is_map_item(&o.result.id)).collect();
    assert!(!maps.is_empty(), "no map offered: {sold:?}");
    for offer in maps {
        let map = &offer.result;
        assert!(component(map, "minecraft:map_id").is_some(), "{} without a number", map.id);
        let target = component(map, "minecraft:map_decorations").and_then(|d| d.get("+"));
        assert!(target.is_some(), "{} without its target", map.id);
        assert_eq!(offer.cost_b.as_ref().map(|c| c.id.as_str()), Some("minecraft:compass"), "{}", map.id);
    }
}

/// With known structures skipped, the structure the origin stands in takes a reference before
/// the search (a cartographer's map leads away from its own village); without skipping, none.
#[test]
fn the_structure_of_the_origin_is_designated_first() {
    let Some((mut s, _dir)) = crate::portal_tests::open_new_world("exploration-claim", SEED) else { return };
    s.load_now(&[(0, 0)]);
    let village = "minecraft:village_plains";
    let start = Nbt::Compound(vec![
        ("id".into(), Nbt::String(village.into())),
        ("ChunkX".into(), Nbt::Int(0)),
        ("ChunkZ".into(), Nbt::Int(0)),
        ("references".into(), Nbt::Int(0)),
        // Two pieces: the origin lies in neither, but in the box of the whole start.
        ("Children".into(), Nbt::List(vec![piece([0, 60, 0, 3, 70, 3]), piece([10, 60, 10, 14, 70, 14])])),
    ]);
    let structures = Nbt::Compound(vec![
        ("References".into(), Nbt::Compound(vec![(village.into(), Nbt::LongArray(vec![0]))])),
        ("starts".into(), Nbt::Compound(vec![(village.into(), start)])),
    ]);
    let origin = [6, 65, 6];
    let references = |s: &crate::server::Server| {
        let Some(Some(chunk)) = s.chunks.get(&(0, 0)) else { panic!("chunk 0, 0 loaded") };
        let start = chunk.root.get("structures").and_then(|t| t.get("starts")).and_then(|t| t.get(village));
        start.and_then(|t| crate::nbtio::int(t, "references"))
    };
    let try_with = |s: &mut crate::server::Server, skip: bool| {
        let Some(Some(chunk)) = s.chunks.get_mut(&(0, 0)) else { panic!("chunk 0, 0 loaded") };
        crate::nbtio::set(&mut chunk.root, "structures", structures.clone());
        let settings = ExplorationSettings::of_function(&json!({
            "destination": "#minecraft:on_plains_village_maps",
            "search_radius": 1,
            "skip_existing_chunks": skip,
        }));
        let mut map = crate::player::Stack { id: "minecraft:plains_village_map".into(), count: 1, components: None };
        s.make_exploration_map(&mut map, &settings, origin);
    };
    try_with(&mut s, false);
    assert_eq!(references(&s), Some(0), "nothing designated without skipping");
    try_with(&mut s, true);
    assert_eq!(references(&s), Some(1), "the start the origin stands in designated");
}

/// The whole box of a start is 12 blocks wider than its pieces when the structure adapts the
/// terrain: an origin 5 blocks beside a village's pieces still claims the village, while the
/// same spot beside a buried treasure (no adaptation) claims nothing.
#[test]
fn the_origin_structure_box_includes_the_terrain_margin() {
    let Some((mut s, _dir)) = crate::portal_tests::open_new_world("exploration-claim-margin", SEED) else { return };
    s.load_now(&[(0, 0)]);
    // Pieces spread along x, the origin 5 blocks past them along z, same chunk.
    let pieces = [[0, 60, 0, 3, 70, 3], [10, 60, 0, 14, 70, 3]];
    let origin = [6, 65, 8];
    let claimed = |s: &mut crate::server::Server, structure: &str| {
        let start = Nbt::Compound(vec![
            ("id".into(), Nbt::String(structure.into())),
            ("ChunkX".into(), Nbt::Int(0)),
            ("ChunkZ".into(), Nbt::Int(0)),
            ("references".into(), Nbt::Int(0)),
            ("Children".into(), Nbt::List(pieces.iter().map(|&bb| piece(bb)).collect())),
        ]);
        let structures = Nbt::Compound(vec![
            ("References".into(), Nbt::Compound(vec![(structure.into(), Nbt::LongArray(vec![0]))])),
            ("starts".into(), Nbt::Compound(vec![(structure.into(), start)])),
        ]);
        let Some(Some(chunk)) = s.chunks.get_mut(&(0, 0)) else { panic!("chunk 0, 0 loaded") };
        crate::nbtio::set(&mut chunk.root, "structures", structures);
        s.claim_structure_at(origin, &[structure.to_owned()]);
        let Some(Some(chunk)) = s.chunks.get(&(0, 0)) else { panic!("chunk 0, 0 loaded") };
        let start = chunk.root.get("structures").and_then(|t| t.get("starts")).and_then(|t| t.get(structure));
        start.and_then(|t| crate::nbtio::int(t, "references"))
    };
    assert_eq!(claimed(&mut s, "minecraft:village_plains"), Some(1), "village box grown by 12 holds the origin");
    assert_eq!(claimed(&mut s, "minecraft:buried_treasure"), Some(0), "treasure box (no adaptation) does not");
}

/// `/loot` runs the exploration map function from its origin, like a container's loot: the
/// shipwreck's treasure map comes out made (a number, a target, no marker left behind).
#[test]
fn loot_command_makes_the_exploration_map() {
    let Some((mut s, _dir)) = crate::portal_tests::open_new_world("loot-command-map", SEED) else { return };
    s.slots.fill(None);
    // Drawn beside a buried treasure, well within the map's search radius.
    let wanted = ["minecraft:buried_treasure".to_owned()];
    let (treasure, _) = s.structure_search(&wanted, [0, 64, 0], 100).expect("a buried treasure");
    let src = s.server_source();
    let command = format!(
        "execute positioned {} 64 {} run loot give @a loot minecraft:chests/shipwreck_map",
        treasure[0] + 16,
        treasure[2]
    );
    let (ok, _) = s.perform(src, &command);
    assert!(ok, "loot command refused: {command}");
    let map = s.slots.iter().flatten().find(|st| st.id == "minecraft:buried_treasure_map").expect("treasure map given");
    assert!(component(map, "minecraft:map_id").is_some(), "map without a number");
    assert!(component(map, "minecraft:map_decorations").and_then(|d| d.get("+")).is_some(), "map without its target");
    assert!(component(map, "minecraft:custom_data").is_none(), "marker left on the map");
}

fn piece(bb: [i32; 6]) -> Nbt {
    Nbt::Compound(vec![("BB".into(), Nbt::IntArray(bb.to_vec()))])
}
