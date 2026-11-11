//! Map markers: the marker types counted towards the banner cap, the spinning rotation of the
//! markers of a Nether map, a banner changed in the world taking the place of the recorded one.
//! Fresh flat world (player at the origin, first air layer at y = -60); the 26.3 data cache and
//! the client jar are required: the tests fail without them.

use rc_proto::nbt::Nbt;

use super::{Banner, Deco, MapData};
use crate::bench_kit::cmd;
use crate::blocks::probe::Probe;
use crate::items::set_component;
use crate::player::Stack;

/// Marker types left out of the count: the targets of exploration maps.
const UNCOUNTED_MARKER_TYPES: [&str; 18] = [
    "target_x",
    "target_point",
    "mansion",
    "monument",
    "red_x",
    "village_desert",
    "village_plains",
    "village_savanna",
    "village_snowy",
    "village_taiga",
    "jungle_temple",
    "swamp_hut",
    "trial_chambers",
    "abandoned_camp",
    "ancient_city",
    "desert_pyramid",
    "mineshaft",
    "ocean_ruin_warm",
];

#[test]
fn only_players_frames_plain_markers_and_banners_are_counted() {
    let data = rc_testdata::version_data().expect("26.3 data cache missing");
    let types = &data.registry("minecraft:map_decoration_type").expect("marker type registry").entries;
    // A type added by a later version of the game has to be classified here first.
    assert_eq!(types.len(), 40, "marker types of the game: {types:?}");
    for name in UNCOUNTED_MARKER_TYPES {
        assert!(types.iter().any(|t| t.strip_prefix("minecraft:") == Some(name)), "{name} is not a marker type");
    }
    for full_name in types {
        let name = full_name.strip_prefix("minecraft:").expect("namespaced type");
        let counted = !UNCOUNTED_MARKER_TYPES.contains(&name);
        assert_eq!(MapData::counts_as_tracked(full_name), counted, "{full_name}");
    }
}

const OVERWORLD: &str = "minecraft:overworld";
const NETHER: &str = "minecraft:the_nether";
const BANNER_AT: [i32; 3] = [3, -60, 3];
const BANNER_KEY: &str = "banner-3,-60,3";
/// First slot of the hotbar.
const HELD_SLOT: usize = 36;
/// Rotation of a marker turned half a turn (banners, exploration targets).
const HALF_TURN: u8 = 8;

fn place_banner(p: &mut Probe, color: &str) {
    cmd(p, &format!("setblock {} {} {} minecraft:{color}_banner", BANNER_AT[0], BANNER_AT[1], BANNER_AT[2]));
}

/// A new map of `dimension` centred on the origin, held in the first hotbar slot; its number
/// and its item.
fn hold_new_map(p: &mut Probe, dimension: &str) -> (i32, Stack) {
    let id = p.s.map_create(0.0, 0.0, 0, true, false);
    p.s.maps.maps.get_mut(&id).expect("new map").dim = dimension.to_owned();
    let mut stack = Stack { id: "minecraft:filled_map".into(), count: 1, components: None };
    set_component(&mut stack, "minecraft:map_id", Nbt::Int(id));
    p.s.slots[HELD_SLOT] = Some(stack.clone());
    (id, stack)
}

fn map(p: &Probe, id: i32) -> &MapData {
    p.s.maps.maps.get(&id).expect("map")
}

fn marker<'a>(map: &'a MapData, key: &str) -> Option<&'a Deco> {
    map.decorations.iter().find(|(k, _)| k == key).map(|(_, marker)| marker)
}

/// The item `stack` with one exploration target of type `kind`, keyed `+` like the game's.
fn with_target(mut stack: Stack, kind: &str) -> Stack {
    let target = Nbt::Compound(vec![
        ("type".into(), Nbt::String(kind.into())),
        ("x".into(), Nbt::Double(10.0)),
        ("z".into(), Nbt::Double(-20.0)),
        ("rotation".into(), Nbt::Float(180.0)),
    ]);
    set_component(&mut stack, "minecraft:map_decorations", Nbt::Compound(vec![("+".into(), target)]));
    stack
}

/// Spinning rotation of the game at three game times, computed apart with 32-bit wrapping
/// integers (the third time overflows the 32-bit tenth of the time).
const SPINS: [(i64, u8); 3] = [(77_770, 4), (5_000, 11), (123_456_789_012, 11)];

#[test]
fn banner_and_target_markers_of_a_nether_map_spin_with_the_game_time() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("map-marker-spin");
    place_banner(&mut p, "red");
    for (game_time, spin) in SPINS {
        p.s.time = game_time;
        let (nether, stack) = hold_new_map(&mut p, NETHER);
        assert!(p.s.map_use_on_banner(HELD_SLOT, BANNER_AT));
        p.s.map_carried(nether, &with_target(stack, "minecraft:red_x"));
        let nether_map = map(&p, nether);
        assert_eq!(marker(nether_map, BANNER_KEY).map(|m| m.rot), Some(spin), "banner at time {game_time}");
        assert_eq!(marker(nether_map, "+").map(|m| m.rot), Some(spin), "target at time {game_time}");
        // Read back from the save, the banner marker has the angle of a banner again.
        let reread = MapData::from_nbt(&nether_map.to_nbt()).expect("saved map");
        assert_eq!(marker(&reread, BANNER_KEY).map(|m| m.rot), Some(HALF_TURN), "banner read back");

        let (overworld, stack) = hold_new_map(&mut p, OVERWORLD);
        assert!(p.s.map_use_on_banner(HELD_SLOT, BANNER_AT));
        p.s.map_carried(overworld, &with_target(stack, "minecraft:red_x"));
        let overworld_map = map(&p, overworld);
        assert_eq!(marker(overworld_map, BANNER_KEY).map(|m| m.rot), Some(HALF_TURN), "overworld banner");
        assert_eq!(marker(overworld_map, "+").map(|m| m.rot), Some(HALF_TURN), "overworld target");
    }
}

#[test]
fn mineshaft_target_is_not_counted_towards_the_banner_cap() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("map-marker-count");
    let (id, stack) = hold_new_map(&mut p, NETHER);
    p.s.map_carried(id, &with_target(stack, "minecraft:mineshaft"));
    let held = map(&p, id);
    assert_eq!(marker(held, "+").map(|m| m.kind.as_str()), Some("minecraft:mineshaft"));
    assert_eq!(held.tracked, 0, "the mineshaft target is counted");
}

fn toggle_banner(p: &mut Probe) {
    assert!(p.s.map_use_on_banner(HELD_SLOT, BANNER_AT));
}

/// What the map records for the banner: its entries (colour, name), its marker (type, name), the count.
type RecordedBanner = (Vec<(String, Option<Nbt>)>, Option<(String, Option<Nbt>)>, i32);

fn recorded_banner(map: &MapData) -> RecordedBanner {
    let entries = map.banners.iter().map(|b| (b.color.clone(), b.name.clone())).collect();
    (entries, marker(map, BANNER_KEY).map(|m| (m.kind.clone(), m.name.clone())), map.tracked)
}

fn one_banner(color: &str, name: Option<Nbt>) -> RecordedBanner {
    (vec![(color.to_owned(), name.clone())], Some((format!("minecraft:banner_{color}"), name)), 1)
}

#[test]
fn banner_changed_in_the_world_replaces_the_recorded_one() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("map-banner-replace");
    // In the Nether no player marker joins the count (the player stands in the Overworld).
    let (id, _) = hold_new_map(&mut p, NETHER);
    place_banner(&mut p, "red");
    toggle_banner(&mut p);
    assert_eq!(recorded_banner(map(&p, id)), one_banner("red", None), "first click");

    // Another colour at the same place: one entry, one marker, the count unchanged.
    place_banner(&mut p, "blue");
    toggle_banner(&mut p);
    assert_eq!(recorded_banner(map(&p, id)), one_banner("blue", None), "repainted banner");

    // Named since: the named banner takes the place of the plain one.
    let name = Nbt::String("Base".into());
    let block_entity = p.s.block_entity_mut(BANNER_AT, "minecraft:banner").expect("banner block entity");
    crate::nbtio::set(block_entity, "CustomName", name.clone());
    toggle_banner(&mut p);
    assert_eq!(recorded_banner(map(&p, id)), one_banner("blue", Some(name.clone())), "renamed banner");

    // The colour sampling of that column finds the banner it recorded: the marker stays.
    p.s.map_check_banners(id, BANNER_AT[0], BANNER_AT[2]);
    assert_eq!(recorded_banner(map(&p, id)), one_banner("blue", Some(name)), "after the column check");
    let saved = map(&p, id).to_nbt();
    let saved_banners = crate::nbtio::list(saved.get("data").and_then(|d| d.get("banners")));
    assert_eq!(saved_banners.len(), 1, "saved banners: {saved_banners:?}");

    // The same banner clicked again comes off.
    toggle_banner(&mut p);
    assert_eq!(recorded_banner(map(&p, id)), (Vec::new(), None, 0), "second click on the same banner");
}

/// A save holding two banners at one position (written before banners replaced each other):
/// the last one wins, like the marker under their shared key.
#[test]
fn saved_banners_at_one_position_keep_the_last() {
    let mut saved = MapData::fresh(0.0, 0.0, 0, true, false, OVERWORLD);
    let named = Some(Nbt::String("Base".into()));
    saved.banners = vec![
        Banner { pos: BANNER_AT, color: "red".into(), name: None },
        Banner { pos: [9, -60, 9], color: "lime".into(), name: None },
        Banner { pos: BANNER_AT, color: "blue".into(), name: named.clone() },
    ];
    let read = MapData::from_nbt(&saved.to_nbt()).expect("saved map");
    let colors: Vec<&str> = read.banners.iter().map(|b| b.color.as_str()).collect();
    assert_eq!(colors, ["blue", "lime"], "banners read back");
    assert_eq!(marker(&read, BANNER_KEY).map(|m| (m.kind.as_str(), &m.name)), Some(("minecraft:banner_blue", &named)));
    assert_eq!((read.decorations.len(), read.tracked), (2, 2), "markers and count");
}
