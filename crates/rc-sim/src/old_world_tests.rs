//! Worlds of older versions (`rc-datafix`): data converted as it is read, like the game, and
//! data older than the conversion covers never read as empty, regenerated nor written over.

use std::path::Path;

use rc_proto::nbt::Nbt;

use crate::chunk::Skip;
use crate::nbtio;
use crate::region::{RegionFile, local_index, region_name};

const OVERWORLD: &str = "minecraft:overworld";

/// A chunk of before 1.18, everything under `Level`.
fn chunk_of_1_16() -> Nbt {
    Nbt::Compound(vec![
        ("DataVersion".into(), Nbt::Int(2586)),
        ("Level".into(), Nbt::Compound(vec![("Status".into(), Nbt::String("full".into()))])),
    ])
}

/// A chunk older than the conversion covers is refused on its version before its status: never
/// "unfinished" (which has it generated over the player's terrain), with or without a version.
/// The old code read the status first: a 1.16 chunk, whose status sits under `Level`, was
/// unfinished and generated again.
#[test]
fn old_chunks_are_refused_before_their_status() {
    let mut old = chunk_of_1_16();
    assert_eq!(crate::chunk::upgrade(&mut old, OVERWORLD), Err(Skip::OldVersion(2586)));
    assert_eq!(old, chunk_of_1_16(), "an old chunk is left as it is");
    let mut unversioned = Nbt::Compound(vec![("Status".into(), Nbt::String("minecraft:noise".into()))]);
    assert_eq!(crate::chunk::upgrade(&mut unversioned, OVERWORLD), Err(Skip::OldVersion(-1)));
    let Some(data) = rc_testdata::version_data() else { return };
    let states = crate::states::States::new(&data, Vec::new());
    let h = crate::dimension_height(&data, OVERWORLD);
    assert_eq!(crate::chunk::parse(chunk_of_1_16(), &data, &states, h).err(), Some(Skip::OldVersion(2586)));
}

fn write_raw(path: &Path, (x, z): (i32, i32), root: &Nbt) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = RegionFile::open_or_create(path).unwrap();
    f.write(local_index(x, z), x, z, &nbtio::encode_root(root)).unwrap();
    f.close().unwrap();
}

fn read_raw(path: &Path, (x, z): (i32, i32)) -> Option<Vec<u8>> {
    RegionFile::open(path, false).ok()?.read(local_index(x, z), x, z).ok().flatten()
}

/// Entity chunks are read converted when older; one older than the conversion covers is not
/// read and listed apart (the server never writes it over). The old code read both as they were.
#[test]
fn entity_chunks_are_read_converted() {
    let Some(data) = rc_testdata::version_data() else { return };
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-old-entities-{}", std::process::id())));
    let dim = crate::world::Dimension::new(&world, OVERWORLD, crate::dimension_height(&data, OVERWORLD));
    let path = dim.dir.join("entities").join(region_name(0, 0));
    let cart = snbt(
        r#"{DataVersion: 4950, Position: [I; 0, 0], Entities: [{id: "minecraft:chest_minecart",
            DisplayState: {Name: "minecraft:chest"}, Items: [{Slot: 0b, id: "minecraft:woodland_explorer_map", count: 1}]}]}"#,
    );
    write_raw(&path, (0, 0), &cart);
    write_raw(&path, (1, 0), &snbt(r#"{DataVersion: 3700, Position: [I; 1, 0], Entities: [{id: "minecraft:pig"}]}"#));
    let states = crate::states::States::new(&data, Vec::new());
    let read = crate::loader::Read::of(&dim, &[(0, 0), (1, 0)], &data, &states, usize::MAX);
    assert_eq!(read.too_old_entities, [(1, 0)]);
    let entities = |p| read.entities.iter().find(|(q, _)| *q == p).and_then(|(_, n)| n.clone());
    assert!(entities((1, 0)).is_none(), "an entity chunk too old to convert is not read");
    let converted = entities((0, 0)).expect("converted entity chunk");
    let cart = &nbtio::list(converted.get("Entities"))[0];
    assert_eq!(cart.get("DisplayState").and_then(|s| s.get("id")).and_then(Nbt::as_str), Some("minecraft:chest"));
    let item = &nbtio::list(cart.get("Items"))[0];
    assert_eq!(item.get("id").and_then(Nbt::as_str), Some("minecraft:woodland_mansion_map"));
    assert_eq!(converted.get("DataVersion"), Some(&Nbt::Int(crate::chunk::DATA_VERSION)));
}

/// An entity file too old to convert is never written over, even when an entity of this session
/// ends in its chunk (its old entities were not read: writing would lose them). The old code read
/// the old entities as they were and wrote them back as current data.
#[test]
fn too_old_entity_file_is_never_written_over() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    let dir = crate::tests::fresh_copy(&world, "old-entity-file");
    let mut p = crate::blocks::probe::Probe::open(&dir, std::sync::Arc::new(data));
    p.s.add_orb([105.5, -59.0, 105.5], 17);
    p.s.add_orb([140.5, -59.0, 105.5], 3);
    p.s.too_old_entity_chunks.insert((6, 6));
    let mut jobs = Vec::new();
    p.s.drops_save_jobs(&mut jobs);
    let written: Vec<(i32, i32)> =
        jobs.iter().filter_map(|j| if let crate::save::Job::Chunk { cx, cz, .. } = j { Some((*cx, *cz)) } else { None }).collect();
    assert!(!written.contains(&(6, 6)), "the old entity file was written over");
    assert!(written.contains(&(8, 6)), "the other chunk's orb is still saved: {written:?}");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// A player saved by 26.2 is read converted: the explorer map in their inventory is the item of
/// 26.3. The old code read the old item identifier.
#[test]
fn players_are_read_converted() {
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-old-player-{}", std::process::id())));
    let uuid = rc_client::offline_uuid("OldTimer");
    let dir = world.join("players").join("data");
    std::fs::create_dir_all(&dir).unwrap();
    let player = snbt(
        r#"{DataVersion: 4950, Inventory: [{Slot: 0b, id: "minecraft:filled_map", count: 1,
            components: {"minecraft:map_decorations": {"+": {type: "minecraft:monument", x: 0.0d, z: 0.0d, rotation: 0.0f}}}}]}"#,
    );
    std::fs::write(dir.join(format!("{}.dat", nbtio::uuid_text(uuid))), nbtio::gzip(&nbtio::encode_root(&player))).unwrap();
    let read = crate::player::PlayerData::read(&world, uuid);
    let first = read.slots.iter().flatten().next().expect("the map");
    assert_eq!(first.id, "minecraft:ocean_monument_map");
}

/// The text form of NBT the conversion tests write cases in.
pub(crate) fn snbt(text: &str) -> Nbt {
    crate::commands::snbt::parse(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// Every chunk of the region file holding `p`, written back as 26.2 wrote them (data version
/// 4950, block states with `Name` and `Properties`).
fn age_region_to_26_2(path: &Path) {
    let mut f = RegionFile::open(path, true).unwrap();
    let present: Vec<usize> = f.present().collect();
    let (rx, rz) = region_of(path);
    for i in present {
        let (x, z) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
        let Ok(Some(bytes)) = f.read(i, x, z) else { continue };
        let mut root = nbtio::parse_root(&bytes).unwrap();
        nbtio::set(&mut root, "DataVersion", Nbt::Int(4950));
        for section in nbtio::get_mut(&mut root, "sections").into_iter().flat_map(list_mut) {
            let palette = nbtio::get_mut(section, "block_states").and_then(|b| nbtio::get_mut(b, "palette"));
            for entry in palette.into_iter().flat_map(list_mut) {
                rename_key(entry, "id", "Name");
                rename_key(entry, "properties", "Properties");
            }
        }
        f.write(i, x, z, &nbtio::encode_root(&root)).unwrap();
    }
    f.close().unwrap();
}

fn region_of(path: &Path) -> (i32, i32) {
    let name = path.file_name().unwrap().to_str().unwrap();
    let mut parts = name.strip_prefix("r.").unwrap().strip_suffix(".mca").unwrap().split('.');
    (parts.next().unwrap().parse().unwrap(), parts.next().unwrap().parse().unwrap())
}

fn list_mut(v: &mut Nbt) -> &mut [Nbt] {
    match v {
        Nbt::List(items) => items,
        _ => &mut [],
    }
}

fn rename_key(v: &mut Nbt, from: &str, to: &str) {
    if let Nbt::Compound(fields) = v
        && let Some((k, _)) = fields.iter_mut().find(|(k, _)| k == from)
    {
        *k = to.to_owned();
    }
}

/// A world last played in 26.2 opens (its `level.dat` and the chunks around the player converted
/// as they load, then saved converted); a chunk there older than the conversion covers, and its
/// entities, are left exactly as they are on the disk. The old code refused the world ("open it
/// first with the official game").
#[test]
fn world_of_26_2_opens_converted_and_keeps_what_is_too_old() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    let dir = crate::tests::fresh_copy(&world, "old-world");
    let level_path = dir.join("level.dat");
    let mut level = nbtio::read_dat(&level_path).unwrap();
    let data_compound = nbtio::get_mut(&mut level, "Data").unwrap();
    nbtio::set(data_compound, "DataVersion", Nbt::Int(4950));
    let spawn = crate::level::Level::from_root(level.clone()).unwrap().spawn_pos;
    std::fs::write(&level_path, nbtio::gzip(&nbtio::encode_root(&level))).unwrap();
    let home = ((spawn[0] >> 4), (spawn[2] >> 4));
    let region = crate::world::dimension_dir(&dir, OVERWORLD).join("region").join(region_name(home.0, home.1));
    age_region_to_26_2(&region);
    // A state with properties, as 26.2 wrote it, in the player's chunk.
    let mut home_old = nbtio::parse_root(&read_raw(&region, home).expect("home chunk")).unwrap();
    let first_palette = nbtio::get_mut(&mut home_old, "sections")
        .and_then(|s| list_mut(s).first_mut())
        .and_then(|s| nbtio::get_mut(s, "block_states"))
        .and_then(|b| nbtio::get_mut(b, "palette"))
        .and_then(|p| list_mut(p).first_mut())
        .expect("a palette");
    *first_palette = snbt(r#"{Name: "minecraft:oak_log", Properties: {axis: "x"}}"#);
    write_raw(&region, home, &home_old);
    // A chunk two away from the player's, and its entities, from 1.20 (too old to convert).
    let old = (home.0 + 2, home.1);
    let entities = crate::world::dimension_dir(&dir, OVERWORLD).join("entities").join(region_name(old.0, old.1));
    let mut old_chunk = nbtio::parse_root(&read_raw(&region_for(&dir, old), old).expect("chunk")).unwrap();
    nbtio::set(&mut old_chunk, "DataVersion", Nbt::Int(3700));
    write_raw(&region_for(&dir, old), old, &old_chunk);
    let old_entities = Nbt::Compound(vec![
        ("DataVersion".into(), Nbt::Int(3700)),
        ("Position".into(), Nbt::IntArray(vec![old.0, old.1])),
        ("Entities".into(), Nbt::List(Vec::new())),
    ]);
    write_raw(&entities, old, &old_entities);
    let before = (read_raw(&region_for(&dir, old), old), read_raw(&entities, old));
    let (session, handle, _) = crate::tests::start_test(&dir, std::sync::Arc::new(data));
    crate::tests::wait_ticks(&session, 40);
    session.link.disconnect();
    handle.wait();
    let home_chunk = nbtio::parse_root(&read_raw(&region, home).expect("home chunk")).unwrap();
    assert_eq!(home_chunk.get("DataVersion"), Some(&Nbt::Int(crate::chunk::DATA_VERSION)), "converted and saved");
    // States without properties are plain text in 26.3 palettes; the others name their block `id`.
    let saved: Vec<Nbt> = saved_chunks(&region).into_iter().filter(|c| c.get("DataVersion") == Some(&Nbt::Int(crate::chunk::DATA_VERSION))).collect();
    let entries: Vec<&Nbt> = saved
        .iter()
        .flat_map(|c| nbtio::list(c.get("sections")))
        .flat_map(|s| nbtio::list(s.get("block_states").and_then(|b| b.get("palette"))))
        .collect();
    assert!(entries.iter().all(|e| e.get("Name").is_none() && e.get("Properties").is_none()), "block states not converted");
    let log = snbt(r#"{id: "minecraft:oak_log", properties: {axis: "x"}}"#);
    let home_entries = nbtio::list(home_chunk.get("sections"))
        .iter()
        .flat_map(|s| nbtio::list(s.get("block_states").and_then(|b| b.get("palette"))))
        .any(|e| *e == log);
    assert!(home_entries, "the player's state with properties not kept converted");
    assert_eq!((read_raw(&region_for(&dir, old), old), read_raw(&entities, old)), before, "too old: left as it was");
    let level = nbtio::read_dat(&level_path).unwrap();
    assert_eq!(level.get("Data").and_then(|d| d.get("DataVersion")), Some(&Nbt::Int(crate::chunk::DATA_VERSION)));
}

/// Every chunk of a region file.
fn saved_chunks(path: &Path) -> Vec<Nbt> {
    let mut f = RegionFile::open(path, false).unwrap();
    let (rx, rz) = region_of(path);
    let present: Vec<usize> = f.present().collect();
    present
        .into_iter()
        .filter_map(|i| f.read(i, rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32).ok().flatten())
        .map(|bytes| nbtio::parse_root(&bytes).unwrap())
        .collect()
}

fn region_for(world: &Path, (x, z): (i32, i32)) -> std::path::PathBuf {
    crate::world::dimension_dir(world, OVERWORLD).join("region").join(region_name(x, z))
}
