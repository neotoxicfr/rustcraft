//! Maps in frames: the frame list read back into markers and written again; a framed map sent
//! to a player who does not carry it, its frame's marker and entry kept; both gone when the
//! frame is killed.

use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::MapFrame;
use crate::blocks::probe::Probe;
use crate::maps::MapData;

#[test]
fn frame_list_is_read_back_into_markers_and_saved_again() {
    let mut m = MapData::fresh(0.0, 0.0, 0, true, false, "minecraft:overworld");
    let frame = MapFrame { pos: [5, 64, -3], rotation: 270, entity_id: 7 };
    let mut root = m.to_nbt();
    if let Some(Nbt::Compound(f)) = crate::nbtio::get_mut(&mut root, "data") {
        f.retain(|(k, _)| k != "frames");
        f.push(("frames".into(), Nbt::List(vec![frame.to_nbt()])));
    }
    m = MapData::from_nbt(&root).expect("map read back");
    assert_eq!(m.frames, vec![frame]);
    let marker = m.decorations.iter().find(|(k, _)| k == "frame-7").map(|(_, d)| d.clone()).expect("frame marker");
    assert_eq!(marker.kind, "minecraft:frame");
    assert_eq!((marker.x, marker.y), (10, -5), "marker at the frame block");
    let saved = m.to_nbt();
    let list = saved.get("data").and_then(|d| d.get("frames")).cloned();
    assert_eq!(list, Some(Nbt::List(vec![frame.to_nbt()])), "frame list saved");
}

fn open(tag: &str) -> (Probe, std::path::PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-frame-maps-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("frame-maps");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.loaded = true;
    p.s.cmd.op = 4;
    p.s.pos = [8.5, -60.0, 12.5];
    for slot in p.s.slots.iter_mut() {
        *slot = None;
    }
    (p, dir)
}

fn run(p: &mut Probe, command: &str) {
    let src = p.s.player_source();
    assert!(p.s.perform(src, command).0, "command refused: {command}");
}

#[test]
fn framed_map_is_sent_and_marked_until_its_frame_is_killed() {
    let (mut p, _dir) = open("framed");
    let map = p.s.map_create(8.0, 8.0, 0, true, false);
    p.run("setblock 7 -59 8 minecraft:stone\n");
    run(
        &mut p,
        &format!(
            "summon minecraft:item_frame 8 -59 8 {{Facing:5b,Item:{{id:\"minecraft:filled_map\",count:1,components:{{\"minecraft:map_id\":{map}}}}}}}"
        ),
    );
    let frame_id = p
        .s
        .entities
        .values()
        .flatten()
        .find(|(_, n)| n.get("id").and_then(Nbt::as_str) == Some("minecraft:item_frame"))
        .map(|(id, _)| *id)
        .expect("frame kept");
    p.take_events();
    p.s.frame_maps_tick();
    let sent = p.take_events().into_iter().any(|e| matches!(e, Event::MapData(d) if d.id == map));
    assert!(sent, "the framed map was not sent to a player who does not carry it");
    let m = p.s.maps.maps.get(&map).expect("map loaded");
    let marker = format!("frame-{frame_id}");
    assert!(m.decorations.iter().any(|(k, d)| *k == marker && d.kind == "minecraft:frame"), "no frame marker");
    assert_eq!(m.frames, vec![MapFrame { pos: [8, -59, 8], rotation: 270, entity_id: frame_id }]);
    // Nine ticks later nothing is due; the tenth shows it again.
    for _ in 0..9 {
        p.s.frame_maps_tick();
        assert!(p.s.maps.frame_maps_due.is_empty(), "framed map due before its 10 ticks");
    }
    p.s.frame_maps_tick();
    assert_eq!(p.s.maps.frame_maps_due.len(), 1, "framed map not due on the tenth tick");
    run(&mut p, "kill @e[type=minecraft:item_frame]");
    let m = p.s.maps.maps.get(&map).expect("map loaded");
    assert!(m.frames.is_empty(), "frame entry kept after the kill");
    assert!(!m.decorations.iter().any(|(k, _)| *k == marker), "frame marker kept after the kill");
}
