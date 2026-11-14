//! Our conversion against the game's own, on the same inputs.
//!
//! Each case below is converted by the official game's converter (`tools/datafix_oracle`), its
//! output stored in `tests/oracle/<case>.nbt`; the test converts the same input with this crate
//! and compares (compound field order aside: the game keeps none). To refresh the expected
//! outputs after changing a case: `cargo test -p rc-datafix --lib -- --ignored write_oracle_inputs`,
//! then `python -I tools/datafix_oracle/oracle.py batch target/datafix-oracle/batch.txt`.

use std::path::PathBuf;

use rc_proto::nbt::Nbt;

use super::snbt::{nbt, show, sorted};
use crate::{Context, FileKind, upgrade};

pub(super) struct Case {
    pub(super) name: &'static str,
    pub(super) kind: FileKind,
    /// The kind's name for the game's converter.
    oracle_kind: &'static str,
    /// A chunk's dimension (its conversion context).
    pub(super) dimension: Option<&'static str>,
    pub(super) input: &'static str,
}

/// A chest of items covering every item rule above 4899, nested in each other.
const ITEMS_4899: &str = r#"[
    {Slot: 0b, id: "minecraft:filled_map", count: 1, components: {
        "minecraft:map_id": 3,
        "minecraft:map_decorations": {"+": {type: "minecraft:mansion", x: 10.0d, z: 20.0d, rotation: 180.0f}},
        "minecraft:item_name": {translate: "filled_map.mansion"}}},
    {Slot: 1b, id: "minecraft:filled_map", count: 1, components: {
        "minecraft:map_decorations": {"+": {type: "monument", x: 1.0d, z: 2.0d, rotation: 0.0f}},
        "minecraft:item_name": {translate: "filled_map.monument", color: "red"}}},
    {Slot: 2b, id: "minecraft:filled_map", count: 1, components: {
        "minecraft:map_decorations": {"+": {type: "minecraft:red_x", x: 1.0d, z: 2.0d, rotation: 0.0f}},
        "minecraft:item_name": {translate: "filled_map.buried_treasure"}}},
    {Slot: 3b, id: "minecraft:map", count: 1, components: {"minecraft:item_name": {translate: "filled_map.buried_treasure"}}},
    {Slot: 4b, id: "minecraft:decorated_pot", count: 1, components: {
        "minecraft:pot_decorations": ["minecraft:angler_pottery_sherd", "minecraft:brick", "minecraft:skull_pottery_sherd"]}},
    {Slot: 5b, id: "minecraft:iron_sword", count: 1, components: {
        "minecraft:swing_animation": {type: "stab", duration: 6},
        "minecraft:map_color": 12345,
        "minecraft:block_transformer": {kind: "x"}}},
    {Slot: 6b, id: "minecraft:shulker_box", count: 1, components: {
        "minecraft:container": [{slot: 0, item: {id: "minecraft:filled_map", count: 1, components: {
            "minecraft:map_decorations": {"+": {type: "minecraft:jungle_temple", x: 0.0d, z: 0.0d, rotation: 0.0f}}}}}],
        "minecraft:block_entity_data": {id: "minecraft:shulker_box", Items: []}}},
    {Slot: 7b, id: "minecraft:bundle", count: 1, components: {
        "minecraft:bundle_contents": [{id: "minecraft:trial_explorer_map", count: 1}, {id: "minecraft:woodland_explorer_map", count: 2}]}},
    {Slot: 8b, id: "minecraft:oak_sign", count: 1, components: {
        "minecraft:block_entity_data": {id: "minecraft:sign", front_text: {messages: ["a", "b", "c", "d"]}}}},
    {Slot: 9b, id: "minecraft:written_book", count: 1, components: {
        "minecraft:written_book_content": {title: {raw: "t"}, author: "me", pages: [{raw: {text: "p",
            hover_event: {action: "show_item", id: "minecraft:ocean_explorer_map", count: 1}}}]}}}
]"#;

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "chunk_full_4899_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: format!(
                    r#"{{DataVersion: 4899, xPos: 1, zPos: -2, yPos: -4, Status: "minecraft:full", LastUpdate: 7L,
                    blending_data: {{min_section: 0, max_section: 16, heights: [1.0d, 2.5d, 1.7976931348623157E308d]}},
                    Heightmaps: {{WORLD_SURFACE: [L; 1, 2, 3]}},
                    sections: [
                        {{Y: -1b, block_states: {{palette: [{{Name: "minecraft:stone"}}, {{Name: "minecraft:oak_stairs", Properties: {{facing: "north", half: "top"}}}}], data: [L; 0, 1, 2, 3]}},
                            biomes: {{palette: ["minecraft:plains"]}}}},
                        {{Y: 0b, block_states: {{palette: [{{Name: "minecraft:air"}}]}}}}],
                    block_entities: [
                        {{id: "minecraft:chest", x: 16, y: 64, z: -32, Items: {ITEMS_4899}}},
                        {{id: "minecraft:decorated_pot", x: 17, y: 64, z: -32, sherds: ["minecraft:arms_up_pottery_sherd", "minecraft:brick"]}},
                        {{id: "minecraft:decorated_pot", x: 18, y: 64, z: -32}},
                        {{id: "minecraft:decorated_pot", x: 19, y: 64, z: -32, sherds: ["", "minecraft:brick"]}},
                        {{id: "minecraft:sign", x: 16, y: 65, z: -32, front_text: {{messages: ["", "", "", ""]}}}},
                        {{id: "minecraft:hanging_sign", x: 16, y: 66, z: -32}},
                        {{id: "minecraft:piston", x: 16, y: 67, z: -32, blockState: {{Name: "minecraft:sticky_piston", Properties: {{facing: "up"}}}}}}],
                    block_ticks: [{{i: "minecraft:water", x: 16, y: 64, z: -32, t: 3, p: 0}}],
                    entities: []}}"#
                )
            .leak(),
        },
        Case {
            name: "chunk_noise_4899_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: r#"{DataVersion: 4899, xPos: 3, zPos: 3, Status: "minecraft:noise",
                Heightmaps: {OCEAN_FLOOR_WG: [L; 5]}, blending_data: {min_section: -4, max_section: 20},
                sections: [{Y: -2b, block_states: {palette: [{Name: "minecraft:deepslate"}]}}, {Y: 3b, block_states: {palette: [{Name: "minecraft:stone"}]}}]}"#,
        },
        Case {
            name: "chunk_retrogen_carvers_4950_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: r#"{DataVersion: 4950, xPos: 0, zPos: 0, Status: "minecraft:carvers",
                below_zero_retrogen: {target_status: "minecraft:full", missing_bedrock: [L; 0]},
                Heightmaps: {}, sections: [{Y: -1b, block_states: {palette: [{Name: "minecraft:stone"}]}}, {Y: 0b, block_states: {palette: [{Name: "minecraft:stone"}]}}]}"#,
        },
        Case {
            name: "chunk_retrogen_surface_target_4950_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: r#"{DataVersion: 4950, xPos: 0, zPos: 1, Status: "minecraft:structure_starts",
                below_zero_retrogen: {target_status: "surface"},
                sections: [{Y: -1b, block_states: {palette: [{Name: "minecraft:stone"}]}}, {Y: 2b, block_states: {palette: [{Name: "minecraft:stone"}]}}]}"#,
        },
        Case {
            name: "chunk_full_4899_nether",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:the_nether"),
            input: r#"{DataVersion: 4899, xPos: 0, zPos: 0, Status: "full", blending_data: {min_section: 0, max_section: 8, heights: [3]},
                sections: [{Y: 0b, block_states: {palette: [{Name: "netherrack"}]}}]}"#,
        },
        Case {
            name: "chunk_entities_4899_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: r#"{DataVersion: 4899, xPos: 0, zPos: 0, Status: "minecraft:full",
                entities: [{id: "minecraft:item_frame", Item: {id: "minecraft:filled_map", count: 1,
                    components: {"minecraft:map_decorations": {"+": {type: "minecraft:swamp_hut", x: 0.0d, z: 0.0d, rotation: 0.0f}}}}}]}"#,
        },
        Case {
            name: "chunk_5012_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: r#"{DataVersion: 5012, xPos: 0, zPos: 0, Status: "minecraft:carvers",
                block_entities: [{id: "minecraft:barrel", Items: [{Slot: 0b, id: "minecraft:swamp_explorer_map", count: 1,
                    components: {"minecraft:block_transformer": {}, "minecraft:map_color": 1}}]}]}"#,
        },
        Case {
            name: "entity_chunk_4950",
            kind: FileKind::EntityChunk,
            oracle_kind: "entity_chunk",
            dimension: None,
            input: r#"{DataVersion: 4950, Position: [I; 0, 0], Entities: [
                {id: "minecraft:chest_minecart", DisplayState: {Name: "minecraft:chest", Properties: {facing: "west"}},
                    Items: [{Slot: 0b, id: "minecraft:filled_map", count: 1, components: {"minecraft:map_decorations": {"+": {type: "village_taiga"}}}}],
                    Passengers: [{id: "minecraft:zombie", equipment: {mainhand: {id: "minecraft:decorated_pot", count: 1,
                        components: {"minecraft:pot_decorations": ["minecraft:heart_pottery_sherd"]}}},
                        Passengers: [{id: "minecraft:falling_block", BlockState: {Name: "minecraft:sand"},
                            TileEntityData: {id: "minecraft:hanging_sign"}}]}]},
                {id: "minecraft:villager", Offers: {Recipes: [{buy: {id: "minecraft:emerald", count: 3},
                    sell: {id: "minecraft:filled_map", count: 1, components: {"minecraft:map_decorations": {"+": {type: "minecraft:trial_chambers"}},
                        "minecraft:item_name": {translate: "filled_map.trial_chambers"}}}}]}},
                {id: "minecraft:enderman", carriedBlockState: {Name: "minecraft:grass_block", Properties: {snowy: "false"}}}]}"#,
        },
        Case {
            name: "player_4899",
            kind: FileKind::Player,
            oracle_kind: "player",
            dimension: None,
            input: r#"{DataVersion: 4899, Inventory: [{Slot: 0b, id: "minecraft:filled_map", count: 1,
                    components: {"minecraft:map_decorations": {"+": {type: "minecraft:village_desert"}}, "minecraft:item_name": {translate: "filled_map.village_desert"}}}],
                EnderItems: [{Slot: 0b, id: "minecraft:map", count: 4, components: {"minecraft:item_name": {translate: "filled_map.buried_treasure"}}}],
                equipment: {offhand: {id: "minecraft:iron_axe", count: 1, components: {"minecraft:swing_animation": {type: "whack"}}}},
                RootVehicle: {Attach: [I; 1, 2, 3, 4], Entity: {id: "minecraft:oak_chest_boat",
                    Items: [{Slot: 0b, id: "minecraft:woodland_explorer_map", count: 1}]}},
                ShoulderEntityLeft: {id: "minecraft:parrot", Variant: 1}}"#,
        },
        Case {
            name: "poi_chunk_4900",
            kind: FileKind::PoiChunk,
            oracle_kind: "poi_chunk",
            dimension: None,
            input: r#"{DataVersion: 4900, Sections: {"0": {Valid: 1b, Records: [{type: "minecraft:home", pos: [I; 1, 2, 3], free_tickets: 1}]}}}"#,
        },
        Case {
            name: "level_4950",
            kind: FileKind::Level,
            oracle_kind: "level",
            dimension: None,
            input: r#"{DataVersion: 4950, LevelName: "Old world", version: 19133, Version: {Id: 4950, Name: "26.2", Series: "main", Snapshot: 0b}}"#,
        },
        Case {
            name: "chunk_unknown_entity_4899_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: r#"{DataVersion: 4899, xPos: 0, zPos: 0, Status: "minecraft:full",
                entities: [{id: "othermod:creature"}, {id: "minecraft:item_frame", Item: {id: "minecraft:filled_map", count: 1,
                    components: {"minecraft:map_decorations": {"+": {type: "minecraft:monument"}}}}}],
                block_entities: [{id: "othermod:machine", components: {"minecraft:map_color": 3}}, {id: "minecraft:sign"}]}"#,
        },
        Case {
            name: "structure_4950",
            kind: FileKind::Structure,
            oracle_kind: "structure",
            dimension: None,
            input: r#"{DataVersion: 4950, size: [I; 1, 1, 1], palette: [{Name: "minecraft:oak_sign", Properties: {rotation: "0"}}],
                palettes: [[{Name: "minecraft:stone"}]],
                blocks: [{pos: [I; 0, 0, 0], state: 0, nbt: {id: "minecraft:sign"}}, {pos: [I; 0, 0, 0], state: 0, nbt: {id: "minecraft:decorated_pot"}}],
                entities: [{pos: [0.5d, 0.0d, 0.5d], blockPos: [I; 0, 0, 0], nbt: {id: "minecraft:glow_item_frame",
                    Item: {id: "minecraft:filled_map", count: 1, components: {"minecraft:map_decorations": {"+": {type: "minecraft:village_snowy"}}}}}}]}"#,
        },
        Case {
            name: "hotbar_4950",
            kind: FileKind::Hotbar,
            oracle_kind: "hotbar",
            dimension: None,
            input: r#"{DataVersion: 4950, "0": [{id: "minecraft:swamp_explorer_map", count: 1}, "minecraft:jungle_explorer_map", {}],
                "1": [{id: "minecraft:map", count: 1, components: {"minecraft:item_name": {translate: "filled_map.buried_treasure"}}}]}"#,
        },
        Case {
            name: "scoreboard_4950",
            kind: FileKind::SavedData(crate::SavedData::Scoreboard),
            oracle_kind: "saved_data_scoreboard",
            dimension: None,
            input: r#"{DataVersion: 4950, data: {Objectives: [
                {Name: "maps", CriteriaName: "minecraft.used:minecraft.woodland_explorer_map", DisplayName: "Maps", RenderType: "integer"},
                {Name: "kills", CriteriaName: "killed:zombie", DisplayName: {text: "K"}},
                {Name: "plain", CriteriaName: "dummy", DisplayName: "d"}],
                Teams: [{Name: "t", DisplayName: {text: "T", hover_event: {action: "show_item", id: "minecraft:trial_explorer_map"}}}]}}"#,
        },
        Case {
            name: "stats_4950",
            kind: FileKind::Stats,
            oracle_kind: "stats",
            dimension: None,
            input: r#"{DataVersion: 4950, stats: {"minecraft:used": {"minecraft:jungle_explorer_map": 3, stone: 1},
                "minecraft:custom": {"minecraft:jump": 9, walk_one_cm: 4}}}"#,
        },
        Case {
            name: "debug_profile_4950",
            kind: FileKind::DebugProfile,
            oracle_kind: "debug_profile",
            dimension: None,
            input: r#"{DataVersion: 4950, custom: {"minecraft:post_effect": "always_on", "minecraft:fps": "in_f3"}}"#,
        },
        Case {
            name: "chunk_unnamespaced_ids_4899_overworld",
            kind: FileKind::Chunk,
            oracle_kind: "chunk",
            dimension: Some("minecraft:overworld"),
            input: r#"{DataVersion: 4899, xPos: 0, zPos: 0, Status: "full",
                sections: [{Y: 0b, block_states: {palette: [{Name: "stone"}]}, biomes: {palette: ["plains"]}}],
                block_ticks: [{i: "water", x: 0, y: 0, z: 0, t: 1, p: 0}],
                block_entities: [{id: "chest", Items: [{Slot: 0b, id: "stick", count: 1},
                    {Slot: 1b, id: "minecraft:bundle", count: 1, components: {"minecraft:bundle_contents": ["minecraft:woodland_explorer_map"]}}]}],
                entities: [{id: "item_frame", Item: {id: "filled_map", count: 1}}]}"#,
        },
    ]
}

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn expected_path(case: &Case) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("oracle").join(format!("{}.nbt", case.name))
}

fn read_root(bytes: &[u8]) -> Nbt {
    let mut reader = rc_proto::Reader::new(bytes);
    rc_proto::nbt::read_named(&mut reader).expect("readable NBT").expect("a root").1
}

fn without_version(value: &Nbt) -> Nbt {
    let mut v = value.clone();
    crate::fixes::remove(&mut v, "DataVersion");
    v
}

/// Writes every case's input and the converter's batch list under `target/datafix-oracle`.
#[test]
#[ignore = "writes the inputs of the game's converter (tools/datafix_oracle)"]
fn write_oracle_inputs() {
    let dir = workspace().join("target").join("datafix-oracle");
    std::fs::create_dir_all(&dir).unwrap();
    let mut batch = String::new();
    for case in cases() {
        let input = dir.join(format!("{}.in.nbt", case.name));
        let mut bytes = Vec::new();
        rc_proto::nbt::write_named(&mut bytes, "", &nbt(case.input));
        std::fs::write(&input, bytes).unwrap();
        let from = crate::data_version(&nbt(case.input), case.kind);
        batch.push_str(&format!(
            "{} {from} {} {} {}\n",
            case.oracle_kind,
            input.display(),
            expected_path(&case).display(),
            case.dimension.unwrap_or("")
        ));
    }
    std::fs::create_dir_all(expected_path(&cases()[0]).parent().unwrap()).unwrap();
    std::fs::write(dir.join("batch.txt"), batch).unwrap();
}

/// Every case converts as the game converts it, and ends at the current data version.
#[test]
fn conversions_match_the_games() {
    let mut differences = Vec::new();
    for case in cases() {
        let path = expected_path(&case);
        let bytes = std::fs::read(&path).unwrap_or_else(|_| {
            panic!("{}: expected output missing, regenerate it (see this module's documentation)", path.display())
        });
        let expected = sorted(&without_version(&read_root(&bytes)));
        let mut ours = nbt(case.input);
        let outcome = upgrade(case.kind, &mut ours, &Context { dimension: case.dimension, generator: None });
        assert!(matches!(outcome, crate::Outcome::Upgraded(_)), "{}: {outcome:?}", case.name);
        assert_eq!(ours.get("DataVersion"), Some(&Nbt::Int(crate::CURRENT_VERSION)), "{}", case.name);
        let ours = sorted(&without_version(&ours));
        if ours != expected {
            differences.push(format!("{}:\n  game: {}\n  ours: {}", case.name, show(&expected), show(&ours)));
        }
    }
    assert!(differences.is_empty(), "conversions differ from the game's:\n{}", differences.join("\n"));
}
