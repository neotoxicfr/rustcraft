//! `level.dat` version fields rewritten at every save like the game's: a world last saved by
//! another version or server shows this one afterwards (the world list reads `Version`).

use rc_proto::nbt::Nbt;

use crate::chunk::DATA_VERSION;

fn text(t: &str) -> Nbt {
    Nbt::String(t.to_owned())
}

fn written(data: Vec<(String, Nbt)>) -> Nbt {
    let mut d = Nbt::Compound(data);
    super::write_level_version(&mut d);
    d
}

/// A `Data` compound from another version and server: every version field replaced, the brands
/// read kept once each with `vanilla` added last, the history extended because its last entry
/// is another version (an earlier occurrence of ours does not count), unknown fields kept.
#[test]
fn version_fields_follow_the_game() {
    let d = written(vec![
        ("Version".into(), Nbt::Compound(vec![("Name".into(), text("26.2")), ("Id".into(), Nbt::Int(4000))])),
        ("DataVersion".into(), Nbt::Int(4000)),
        ("version".into(), Nbt::Int(1)),
        ("ServerBrands".into(), Nbt::List(vec![text("fabric"), text("fabric"), Nbt::Int(3), text("paper")])),
        ("WasModded".into(), Nbt::Byte(1)),
        ("version_history".into(), Nbt::List(vec![Nbt::Int(DATA_VERSION), Nbt::Long(4100), text("x")])),
        ("LevelName".into(), text("Zoé")),
    ]);
    let version = Nbt::Compound(vec![
        ("Snapshot".into(), Nbt::Byte(0)),
        ("Series".into(), text("main")),
        ("Id".into(), Nbt::Int(DATA_VERSION)),
        ("Name".into(), text("26.3")),
    ]);
    assert_eq!(d.get("Version"), Some(&version));
    assert_eq!(d.get("DataVersion"), Some(&Nbt::Int(DATA_VERSION)));
    assert_eq!(d.get("version"), Some(&Nbt::Int(19133)));
    let brands = Nbt::List(vec![text("fabric"), text("paper"), text("vanilla")]);
    assert_eq!(d.get("ServerBrands"), Some(&brands));
    assert_eq!(d.get("WasModded"), Some(&Nbt::Byte(1)), "a modded world stays marked");
    let history = Nbt::List(vec![Nbt::Int(DATA_VERSION), Nbt::Int(4100), Nbt::Int(-1), Nbt::Int(DATA_VERSION)]);
    assert_eq!(d.get("version_history"), Some(&history));
    assert_eq!(d.get("LevelName"), Some(&text("Zoé")));
}

/// A world of this version: nothing grows (history ending with ours, `vanilla` already there);
/// missing fields written (`WasModded` false, history and brands started).
#[test]
fn a_world_of_this_version_stays_the_same() {
    let d = written(vec![
        ("ServerBrands".into(), Nbt::List(vec![text("vanilla")])),
        ("version_history".into(), Nbt::List(vec![Nbt::Int(4000), Nbt::Int(DATA_VERSION)])),
    ]);
    assert_eq!(d.get("ServerBrands"), Some(&Nbt::List(vec![text("vanilla")])));
    assert_eq!(d.get("version_history"), Some(&Nbt::List(vec![Nbt::Int(4000), Nbt::Int(DATA_VERSION)])));
    let bare = written(Vec::new());
    assert_eq!(bare.get("WasModded"), Some(&Nbt::Byte(0)));
    assert_eq!(bare.get("ServerBrands"), Some(&Nbt::List(vec![text("vanilla")])));
    assert_eq!(bare.get("version_history"), Some(&Nbt::List(vec![Nbt::Int(DATA_VERSION)])));
}

/// The save writes them: a world opened with another version's fields comes back with ours.
#[test]
fn the_save_rewrites_the_version_fields() {
    let (mut p, dir) = crate::tests_g3f13::open("level-version");
    if let Some(d) = crate::nbtio::get_mut(&mut p.s.level.root, "Data") {
        crate::nbtio::set(d, "Version", Nbt::Compound(vec![("Name".into(), text("26.2"))]));
        crate::nbtio::set(d, "ServerBrands", Nbt::List(vec![text("fabric")]));
    }
    p.s.save(true);
    let root = crate::nbtio::read_dat(&dir.join("level.dat")).expect("level.dat written");
    let d = root.get("Data").expect("Data");
    assert_eq!(d.get("Version").and_then(|v| v.get("Name")), Some(&text("26.3")));
    assert_eq!(d.get("ServerBrands"), Some(&Nbt::List(vec![text("fabric"), text("vanilla")])));
    crate::tests_g3f13::finish(p, dir);
}
