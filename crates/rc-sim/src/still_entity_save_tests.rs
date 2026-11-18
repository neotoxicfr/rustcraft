//! Displays, interactions and markers kept as compounds hold what the game writes for the
//! entity it reads: the common fields with their defaults, the type's own fields with their
//! defaults and bounds, names read back, unknown fields gone.

use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;

fn open(tag: &str) -> (Probe, std::path::PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-still-save-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("still-save");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.loaded = true;
    p.s.cmd.op = 4;
    p.s.pos = [8.5, -60.0, 8.5];
    (p, dir)
}

fn run(p: &mut Probe, command: &str) {
    let src = p.s.player_source();
    assert!(p.s.perform(src, command).0, "command refused: {command}");
}

/// The kept compound of the only entity of type `ty`.
fn kept(p: &Probe, ty: &str) -> Nbt {
    let mut found = p.s.entities.values().flatten().filter(|(_, n)| n.get("id").and_then(Nbt::as_str) == Some(ty));
    let (_, n) = found.next().unwrap_or_else(|| panic!("no {ty} kept"));
    assert!(found.next().is_none(), "several {ty}");
    n.clone()
}

fn keys(n: &Nbt) -> Vec<&str> {
    match n {
        Nbt::Compound(f) => f.iter().map(|(k, _)| k.as_str()).collect(),
        _ => Vec::new(),
    }
}

const COMMON: [&str; 11] =
    ["id", "Pos", "Motion", "Rotation", "fall_distance", "Fire", "Air", "OnGround", "Invulnerable", "PortalCooldown", "UUID"];

const DISPLAY: [&str; 10] = [
    "transformation",
    "billboard",
    "interpolation_duration",
    "teleport_duration",
    "view_range",
    "shadow_radius",
    "shadow_strength",
    "width",
    "height",
    "glow_color_override",
];

#[test]
fn summoned_text_display_holds_every_field_the_game_writes() {
    let (mut p, _dir) = open("text");
    run(&mut p, "summon minecraft:text_display ~ ~ ~ {text:\"a\",teleport_duration:100,alignment:\"left\",foo:1b}");
    let n = kept(&p, "minecraft:text_display");
    let text = ["text", "line_width", "background", "text_opacity", "shadow", "see_through", "default_background", "alignment"];
    let want: Vec<&str> = COMMON.iter().chain(DISPLAY.iter()).chain(text.iter()).copied().collect();
    assert_eq!(keys(&n), want);
    assert_eq!(n.get("teleport_duration"), Some(&Nbt::Int(59)), "teleport duration bound");
    assert_eq!(n.get("Air"), Some(&Nbt::Short(300)));
    assert_eq!(n.get("billboard").and_then(Nbt::as_str), Some("fixed"));
    assert_eq!(n.get("alignment").and_then(Nbt::as_str), Some("left"));
    assert_eq!(n.get("background"), Some(&Nbt::Int(0x4000_0000)));
    assert_eq!(n.get("text_opacity"), Some(&Nbt::Byte(-1)));
    let scale = n.get("transformation").and_then(|t| t.get("scale")).cloned();
    assert_eq!(scale, Some(Nbt::List(vec![Nbt::Float(1.0); 3])), "identity transformation");
}

#[test]
fn block_display_interaction_and_marker_hold_their_own_fields() {
    let (mut p, _dir) = open("others");
    run(&mut p, "summon minecraft:block_display ~ ~ ~ {block_state:{Name:\"minecraft:oak_stairs\"},billboard:\"nonsense\"}");
    let block = kept(&p, "minecraft:block_display");
    let want: Vec<&str> = COMMON.iter().chain(DISPLAY.iter()).chain(["block_state"].iter()).copied().collect();
    assert_eq!(keys(&block), want);
    let state = block.get("block_state").expect("block state");
    assert_eq!(state.get("Name").and_then(Nbt::as_str), Some("minecraft:oak_stairs"));
    assert_eq!(state.get("Properties").and_then(|s| s.get("half")).and_then(Nbt::as_str), Some("bottom"));
    assert_eq!(block.get("billboard").and_then(Nbt::as_str), Some("fixed"), "unknown name read back as the default");
    run(&mut p, "summon minecraft:interaction ~ ~ ~ {width:2,bar:3}");
    let interaction = kept(&p, "minecraft:interaction");
    let want: Vec<&str> = COMMON.iter().chain(["width", "height", "response"].iter()).copied().collect();
    assert_eq!(keys(&interaction), want);
    assert_eq!(interaction.get("width"), Some(&Nbt::Float(2.0)));
    assert_eq!(interaction.get("height"), Some(&Nbt::Float(1.0)));
    run(&mut p, "summon minecraft:marker ~ ~ ~ {data:{note:\"kept\"},baz:4}");
    let marker = kept(&p, "minecraft:marker");
    let want: Vec<&str> = COMMON.iter().chain(["data"].iter()).copied().collect();
    assert_eq!(keys(&marker), want);
}

/// `/data merge` reads the merged compound again: the bound and the unknown field apply too.
#[test]
fn merged_display_is_read_again() {
    let (mut p, _dir) = open("merge");
    run(&mut p, "summon minecraft:item_display ~ ~ ~ {item:{id:\"minecraft:stone\"}}");
    run(&mut p, "data merge entity @n[type=minecraft:item_display] {teleport_duration:80,qux:1b,item_display:\"gui\"}");
    let n = kept(&p, "minecraft:item_display");
    assert_eq!(n.get("teleport_duration"), Some(&Nbt::Int(59)));
    assert!(n.get("qux").is_none(), "unknown field kept");
    assert_eq!(n.get("item_display").and_then(Nbt::as_str), Some("gui"));
    assert_eq!(n.get("item").and_then(|i| i.get("count")), Some(&Nbt::Int(1)));
    assert!(n.get("start_interpolation").is_none(), "interpolation start saved");
}
