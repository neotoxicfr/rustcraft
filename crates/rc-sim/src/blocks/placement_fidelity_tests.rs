//! Placement and pressing rules checked against the game: rails in a current stay dry, a
//! calibrated sculk sensor faces the gaze, a creaking heart placed between its logs roots at once,
//! a grindstone holds without support, and a pressure plate's press names the entity on it.
//! Before, every one of these differed: rails became water sources, the sensor was turned back to
//! front, the heart stayed uprooted, the grindstone needed a support and fell, and the plate's
//! vibration had no source (no shriek, no warden target).

use super::test_area::{Area, open};
use super::{Dir, P, flag};
use crate::mobs::mob::Ent;
use crate::mobs::recent::vibration::Src;
use crate::player::Stack;

/// Height of the cells right above the test area's stone floor.
const Y: i32 = -59;

/// Places `item` held in the main hand by clicking `face` of `clicked`, the player turned to
/// `yaw` / `pitch`; the state of the cell it went into (`None`: refused).
fn place(a: &mut Area, item: &str, clicked: P, face: Dir, yaw: f32, pitch: f32) -> Option<String> {
    let s = &mut a.p.s;
    (s.yaw, s.pitch, s.game_mode) = (yaw, pitch, 1);
    s.slots[36 + s.held as usize] = Some(Stack { id: item.into(), count: 64, components: None });
    let cell = super::rel(clicked, face);
    // Centre of the clicked face.
    let at = [0, 1, 2].map(|i| f64::from(clicked[i]) + 0.5 + 0.5 * f64::from(face.step()[i]));
    s.place_item(item, clicked, face, at).then(|| a.name(cell))
}

/// Places `item` on the top of the floor block under `cell`.
fn place_on_floor(a: &mut Area, item: &str, cell: P, yaw: f32) -> String {
    place(a, item, [cell[0], cell[1] - 1, cell[2]], Dir::Up, yaw, 30.0).unwrap_or_else(|| panic!("{item} refused"))
}

#[test]
fn rails_are_waterlogged_only_in_source_water() {
    let Some(mut a) = open("rails-current") else { return };
    for (x, rail) in [(104, "minecraft:rail"), (106, "minecraft:powered_rail"), (108, "minecraft:detector_rail")] {
        let current = [x, Y, 110];
        a.set(current, "minecraft:water[level=3]");
        let placed = place_on_floor(&mut a, rail, current, 0.0);
        assert!(placed.contains("waterlogged=false"), "{rail} in a current became a source: {placed}");
        let source = [x, Y, 114];
        a.set(source, "minecraft:water");
        let placed = place_on_floor(&mut a, rail, source, 0.0);
        assert!(placed.contains("waterlogged=true"), "{rail} in source water: {placed}");
    }
}

#[test]
fn calibrated_sculk_sensor_faces_the_horizontal_gaze() {
    let Some(mut a) = open("calibrated-facing") else { return };
    // Yaw 180: looking north.
    let placed = place_on_floor(&mut a, "minecraft:calibrated_sculk_sensor", [110, Y, 110], 180.0);
    assert!(placed.contains("facing=north"), "turned back to front: {placed}");
    assert!(placed.contains("waterlogged=false"), "{placed}");
    a.set([112, Y, 110], "minecraft:water");
    let placed = place_on_floor(&mut a, "minecraft:calibrated_sculk_sensor", [112, Y, 110], 90.0);
    assert!(placed.contains("facing=west") && placed.contains("waterlogged=true"), "{placed}");
}

#[test]
fn creaking_heart_placed_between_its_logs_roots_at_once() {
    let Some(mut a) = open("heart-placed") else { return };
    for (x, time, expected) in [(104, 1000, "dormant"), (108, 18000, "awake")] {
        a.cmd(&format!("time set {time}"));
        assert_eq!(crate::mobs::recent::creaking_active(&a.p.s), expected == "awake", "time {time}");
        a.set([x, Y, 110], "minecraft:pale_oak_log[axis=y]");
        a.set([x, Y + 2, 110], "minecraft:pale_oak_log[axis=y]");
        let placed = place(&mut a, "minecraft:creaking_heart", [x, Y, 110], Dir::Up, 0.0, 30.0).expect("heart refused");
        assert!(placed.contains(&format!("creaking_heart_state={expected}")), "time {time}: {placed}");
    }
    // Without its logs it stays uprooted.
    let placed = place_on_floor(&mut a, "minecraft:creaking_heart", [112, Y, 110], 0.0);
    assert!(placed.contains("creaking_heart_state=uprooted"), "{placed}");
}

#[test]
fn grindstone_holds_without_support() {
    let Some(mut a) = open("grindstone-support") else { return };
    // On a bottom slab (no sturdy top, nothing around): the first gaze direction (down, the
    // clicked face's opposite) is kept, so the floor face, where a lever finds no support.
    a.set([110, Y, 110], "minecraft:stone_slab[type=bottom]");
    let placed = place(&mut a, "minecraft:grindstone", [110, Y, 110], Dir::Up, 180.0, 0.0).expect("grindstone refused");
    assert_eq!(placed, "minecraft:grindstone[face=floor,facing=north]");
    // Its support removed, the grindstone stays.
    let (stone, grindstone) = ([114, Y, 110], [114, Y + 1, 110]);
    a.set(stone, "minecraft:stone");
    a.set(grindstone, "minecraft:grindstone[face=floor,facing=north]");
    let air = a.p.s.air();
    assert!(a.p.s.set_block(stone, air, flag::ALL));
    assert_eq!(a.name(grindstone), "minecraft:grindstone[face=floor,facing=north]", "fell with its support");
}

/// Sources of the `event` game events logged since the last clear.
fn sources(a: &Area, event: &str) -> Vec<Option<Src>> {
    a.p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == event).map(|e| e.2).collect()
}

#[test]
fn pressure_plate_press_names_the_entity_on_it() {
    let Some(mut a) = open("plate-source") else { return };
    let plate = [110, Y, 110];
    a.set(plate, "minecraft:stone_pressure_plate");
    a.p.s.mobs.rec.vib.log.clear();
    a.p.s.pos = [110.5, f64::from(Y), 110.5];
    a.p.s.player_touch_blocks(a.p.s.pos);
    assert!(a.name(plate).contains("powered=true"), "plate not pressed");
    assert_eq!(sources(&a, "minecraft:block_activate"), vec![Some(Src::Living(Ent::Player))]);
    // Released by its scheduled tick once the player has left: no source.
    a.p.s.pos = [100.5, f64::from(Y), 131.5];
    let _ = a.ticks(20);
    assert!(a.name(plate).contains("powered=false"), "plate not released");
    assert_eq!(sources(&a, "minecraft:block_deactivate"), vec![None]);
}
