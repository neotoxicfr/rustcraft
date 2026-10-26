//! S8: projectiles on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`), per-tick readings
//! (`RUSTCRAFT_S8_TRACE`) to compare with the official server (same scenario, `tick freeze` then
//! `tick step 1`). Absent: tests skipped.

use std::time::Duration;

use rc_proto::nbt::Nbt;

use crate::tests::{fresh_copy, start_test, test_world};

/// Copy of the world: player at `pos`, mode `mode`, inventory `inv` (slot, item, count),
/// normal difficulty.
pub(crate) fn prep_world(tag: &str, mode: i32, pos: [f64; 3], inv: &[(i8, &str, i32)]) -> Option<std::path::PathBuf> {
    let world = test_world()?;
    let dir = fresh_copy(&world, tag);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).ok()?;
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        let mut ds = d.get("difficulty_settings").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        crate::nbtio::set(&mut ds, "difficulty", Nbt::String("normal".into()));
        crate::nbtio::set(d, "difficulty_settings", ds);
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).ok()?;
    let items: Vec<Nbt> = inv
        .iter()
        .map(|(slot, id, n)| {
            Nbt::Compound(vec![
                ("Slot".into(), Nbt::Byte(*slot)),
                ("id".into(), Nbt::String((*id).into())),
                ("count".into(), Nbt::Int(*n)),
            ])
        })
        .collect();
    for e in std::fs::read_dir(dir.join("players").join("data")).ok()?.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).ok()?;
        crate::nbtio::set(&mut pl, "playerGameType", Nbt::Int(mode));
        crate::nbtio::set(&mut pl, "Pos", dbl(pos));
        crate::nbtio::set(&mut pl, "Inventory", Nbt::List(items.clone()));
        crate::nbtio::set(&mut pl, "Health", Nbt::Float(20.0));
        crate::nbtio::set(&mut pl, "SelectedItemSlot", Nbt::Int(0));
        crate::nbtio::remove(&mut pl, "equipment");
        let mut ab = pl.get("abilities").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for k in ["flying", "invulnerable"] {
            crate::nbtio::set(&mut ab, k, Nbt::Byte(i8::from(mode == 3 && k == "flying")));
        }
        crate::nbtio::set(&mut ab, "instabuild", Nbt::Byte(i8::from(mode == 1)));
        crate::nbtio::set(&mut ab, "mayfly", Nbt::Byte(i8::from(mode == 1 || mode == 3)));
        crate::nbtio::set(&mut pl, "abilities", ab);
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).ok()?;
    }
    Some(dir)
}

/// Entities written into the entity chunks of the main world.
pub(crate) fn inject(dir: &std::path::Path, ents: &[Nbt]) {
    use std::collections::BTreeMap;
    let mut by: BTreeMap<(i32, i32), Vec<Nbt>> = BTreeMap::new();
    for e in ents {
        let p = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap();
        by.entry(((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4)).or_default().push(e.clone());
    }
    let edir = dir.join("dimensions").join("minecraft").join("overworld").join("entities");
    for ((cx, cz), list) in by {
        let path = edir.join(crate::region::region_name(cx, cz));
        let mut r = crate::region::RegionFile::open_or_create(&path).unwrap();
        let root = Nbt::Compound(vec![
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ("Position".into(), Nbt::IntArray(vec![cx, cz])),
            ("Entities".into(), Nbt::List(list)),
        ]);
        r.write(crate::region::local_index(cx, cz), cx, cz, &crate::nbtio::encode_root(&root)).unwrap();
        r.close().unwrap();
    }
}

/// Entities saved from a chunk.
pub(crate) fn saved(dir: &std::path::Path, cx: i32, cz: i32) -> Vec<Nbt> {
    let path = dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("entities")
        .join(crate::region::region_name(cx, cz));
    let Ok(mut r) = crate::region::RegionFile::open(&path, false) else { return Vec::new() };
    let Ok(Some(b)) = r.read(crate::region::local_index(cx, cz), cx, cz) else { return Vec::new() };
    let root = crate::nbtio::parse_root(&b).unwrap();
    crate::nbtio::list(root.get("Entities")).to_vec()
}

fn mob_nbt(id: &str, pos: [f64; 3], extra: Vec<(String, Nbt)>) -> Nbt {
    let mut f = vec![
        ("id".to_owned(), Nbt::String(format!("minecraft:{id}"))),
        ("Pos".to_owned(), dbl(pos)),
        ("PersistenceRequired".to_owned(), Nbt::Byte(1)),
    ];
    f.extend(extra);
    Nbt::Compound(f)
}

fn dbl(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect())
}

fn proj(n: i32, kind: &str, pos: [f64; 3], mot: [f64; 3], extra: Vec<(String, Nbt)>) -> Nbt {
    let mut f = vec![
        ("id".to_owned(), Nbt::String(format!("minecraft:{kind}"))),
        ("Pos".to_owned(), dbl(pos)),
        ("Motion".to_owned(), dbl(mot)),
        ("Rotation".to_owned(), Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(0.0)])),
        ("UUID".to_owned(), Nbt::IntArray(vec![n, 0, 0, n])),
    ];
    f.extend(extra);
    Nbt::Compound(f)
}

fn stack(id: &str, comps: Option<Nbt>) -> Nbt {
    let mut f = vec![("id".to_owned(), Nbt::String(id.into())), ("count".to_owned(), Nbt::Int(1))];
    if let Some(c) = comps {
        f.push(("components".to_owned(), c));
    }
    Nbt::Compound(f)
}

/// Scenario shared with `S8/official_traj.py`.
fn scenario() -> Vec<Nbt> {
    let water = Nbt::Compound(vec![(
        "minecraft:potion_contents".into(),
        Nbt::Compound(vec![("potion".into(), Nbt::String("minecraft:water".into()))]),
    )]);
    vec![
        proj(1, "arrow", [200.5, -57.0, 200.5], [0.8, 0.45, 0.3], vec![]),
        proj(2, "snowball", [200.5, -57.0, 203.5], [0.6, 0.4, -0.2], vec![]),
        proj(3, "trident", [200.5, -57.0, 206.5], [1.0, 0.3, 0.05], vec![]),
        proj(4, "ender_pearl", [200.5, -57.0, 209.5], [0.4, 0.6, 0.1], vec![]),
        proj(
            5,
            "splash_potion",
            [200.5, -57.0, 212.5],
            [0.3, 0.5, 0.0],
            vec![("Item".into(), stack("minecraft:splash_potion", Some(water)))],
        ),
        proj(6, "experience_bottle", [200.5, -57.0, 215.5], [0.2, 0.6, 0.0], vec![]),
        proj(
            7,
            "firework_rocket",
            [200.5, -59.0, 218.5],
            [0.0, 0.05, 0.0],
            vec![("LifeTime".into(), Nbt::Int(25)), ("FireworksItem".into(), stack("minecraft:firework_rocket", None))],
        ),
        proj(
            8,
            "arrow",
            [190.5, -58.0, 200.5],
            [1.5, 0.0, 0.0],
            vec![("crit".into(), Nbt::Byte(0)), ("damage".into(), Nbt::Double(2.0))],
        ),
        mob_nbt(
            "pig",
            [194.5, -59.0, 200.5],
            vec![("NoAI".into(), Nbt::Byte(1)), ("Rotation".into(), Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(0.0)]))],
        ),
    ]
}

/// Demonstration tool: commands `RUSTCRAFT_S8_CMDS` (separated by `;`) played in the
/// world `RUSTCRAFT_S8_PREP` (commands allowed), then save.
#[test]
#[ignore]
fn prep_demo_world() {
    let (Ok(dir), Ok(cmds)) = (std::env::var("RUSTCRAFT_S8_PREP"), std::env::var("RUSTCRAFT_S8_CMDS")) else { return };
    let Some(data) = rc_testdata::version_data() else { return };
    let dir = std::path::PathBuf::from(dir);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    let (s, handle, _) = start_test(&dir, std::sync::Arc::new(data));
    std::thread::sleep(Duration::from_secs(3));
    for c in cmds.split(';').map(str::trim).filter(|c| !c.is_empty()) {
        s.link.send_command(c).unwrap();
        std::thread::sleep(Duration::from_millis(300));
    }
    std::thread::sleep(Duration::from_secs(2));
    s.link.disconnect();
    handle.wait();
}

/// Waits for an `add_entity` of the given type; returns its identifier and speed.
fn wait_add(s: &rc_client::Session, kind: i32, secs: u64) -> Option<(i32, [f64; 3])> {
    let t = std::time::Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if let Ok(rc_client::Event::Entity(rc_client::EntityEvent::Add(a))) =
            s.events.recv_timeout(Duration::from_millis(20))
            && a.kind == kind
        {
            return Some((a.id, a.velocity));
        }
    }
    None
}

fn kind_of(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:entity_type").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

/// Player weapons end to end: bow (charged then released), crossbow (loaded then fired),
/// trident, snowball, blank map, arrow dispenser.
#[test]
fn player_weapons_end_to_end() {
    let Some(data) = rc_testdata::version_data() else { return };
    let inv = [
        (0, "minecraft:bow", 1),
        (1, "minecraft:crossbow", 1),
        (2, "minecraft:trident", 1),
        (3, "minecraft:snowball", 4),
        (4, "minecraft:map", 1),
        (9, "minecraft:arrow", 16),
    ];
    let Some(dir) = prep_world("s8weap", 0, [200.5, -59.0, 200.5], &inv) else { return };
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    let arrow = kind_of(&data, "minecraft:arrow");
    let trident = kind_of(&data, "minecraft:trident");
    let snowball = kind_of(&data, "minecraft:snowball");
    let (s, handle, _) = start_test(&dir, std::sync::Arc::new(data));
    std::thread::sleep(Duration::from_secs(4));
    let l = &s.link;
    // Bow: held 25 ticks, arrow at full power (|v| ≈ 3).
    l.send_carried_item(0).unwrap();
    l.send_use_item(0, 1, -90.0, -10.0).unwrap();
    std::thread::sleep(Duration::from_millis(1400));
    l.send_player_action(6, [0; 3], 0, 0).unwrap();
    let (_, v) = wait_add(&s, arrow, 3).expect("bow arrow");
    let sp = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    assert!((2.5..3.5).contains(&sp), "arrow speed {sp}");
    // Crossbow: loaded (1.25 s), released, then fired by a second use.
    l.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    l.send_use_item(0, 2, -90.0, -10.0).unwrap();
    std::thread::sleep(Duration::from_millis(1700));
    l.send_player_action(6, [0; 3], 0, 0).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    l.send_use_item(0, 3, -90.0, -10.0).unwrap();
    let (_, v) = wait_add(&s, arrow, 3).expect("crossbow arrow");
    let sp = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    assert!((2.6..3.7).contains(&sp), "crossbow arrow speed {sp}");
    // Trident: held 15 ticks.
    l.send_carried_item(2).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    l.send_use_item(0, 4, -90.0, -10.0).unwrap();
    std::thread::sleep(Duration::from_millis(900));
    l.send_player_action(6, [0; 3], 0, 0).unwrap();
    assert!(wait_add(&s, trident, 3).is_some(), "trident thrown");
    // Snowball.
    l.send_carried_item(3).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    l.send_use_item(0, 5, -90.0, -10.0).unwrap();
    assert!(wait_add(&s, snowball, 3).is_some(), "snowball thrown");
    // Powered arrow dispenser.
    l.send_command("setblock 196 -59 196 minecraft:dispenser[facing=east]").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    l.send_command("item replace block 196 -59 196 container.0 with minecraft:arrow 3").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    l.send_command("setblock 196 -59 195 minecraft:redstone_block").unwrap();
    assert!(wait_add(&s, arrow, 3).is_some(), "dispenser arrow");
    // Blank map: filled map number 0.
    l.send_carried_item(4).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    l.send_use_item(0, 6, -90.0, 0.0).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    s.link.disconnect();
    handle.wait();
    assert!(dir.join("data").join("minecraft").join("maps").join("0.dat").is_file(), "map 0 written");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sealed loot chest read by a comparator: the table is drawn (contents present, table
/// forgotten), without opening.
#[test]
fn comparator_unseals_loot_chest() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(dir) = prep_world("s8seal", 1, [200.5, -59.0, 200.5], &[]) else { return };
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    let (s, handle, _) = start_test(&dir, std::sync::Arc::new(data));
    std::thread::sleep(Duration::from_secs(3));
    let l = &s.link;
    for c in [
        "setblock 200 -59 205 minecraft:chest",
        "data merge block 200 -59 205 {LootTable:\"minecraft:chests/simple_dungeon\",LootTableSeed:42L}",
        "setblock 200 -59 206 minecraft:comparator[facing=north]",
        "setblock 201 -59 206 minecraft:stone",
    ] {
        l.send_command(c).unwrap();
        std::thread::sleep(Duration::from_millis(300));
    }
    std::thread::sleep(Duration::from_millis(500));
    while s.events.try_recv().is_ok() {}
    l.send_command("data get block 200 -59 205").unwrap();
    let mut text = String::new();
    let t = std::time::Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        if let Ok(rc_client::Event::Chat { component, .. }) = s.events.recv_timeout(Duration::from_millis(50)) {
            text = format!("{component:?}");
            if text.contains("data.block.query") {
                break;
            }
        }
    }
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(text.contains("Items") && !text.contains("LootTable"), "{text}");
}

/// Named random sequence: same values as `random value 0..94 minecraft:s8test` on the
/// official server (test world seed).
#[test]
fn named_sequence_matches_official() {
    let tmp = std::env::temp_dir().join("rc-s8-seq");
    let mut s = crate::rng::Sequences::load(&tmp, 6_470_889_051_986_534_830);
    let r = s.get("minecraft:s8test");
    let got: Vec<i32> = (0..3).map(|_| r.next_int_bound(95)).collect();
    assert_eq!(got, vec![7, 59, 47]);
}

/// Fishing loot off the server (jar table, new named sequence): same sequence as the official server
/// when the `RUSTCRAFT_S8_FISH` reading is supplied.
#[test]
fn fishing_loot_offline() {
    let Some(data) = rc_testdata::version_data() else { return };
    let data = std::sync::Arc::new(data);
    let Some(jar) = crate::registries::client_jar(&data) else { return };
    let Ok(jar) = crate::registries::Jar::read(&jar) else { return };
    let regs = crate::registries::load_jar(&jar, &data);
    let pack = crate::datapack::DataPack::from_jar(&jar);
    let items = crate::items::Items::new(data.clone(), &regs.tags);
    let bt = |_: &str, _: &str| false;
    let loot = crate::loot::Loot { pack: &pack, items: &items, block_tag: &bt };
    let rod = crate::player::Stack { id: "minecraft:fishing_rod".into(), count: 1, components: None };
    let ctx = crate::loot::Ctx { tool: Some(Some(&rod)), ..Default::default() };
    let tmp = std::env::temp_dir().join("rc-s8-seq2");
    let mut seqs = crate::rng::Sequences::load(&tmp, 6_470_889_051_986_534_830);
    let got: Vec<String> = (0..40)
        .map(|_| loot.drops("minecraft:gameplay/fishing", &ctx, seqs.get("minecraft:gameplay/fishing")))
        .map(|d| d.first().map_or_else(|| "?".to_owned(), |s| s.id.clone()))
        .collect();
    eprintln!("fishing off the server: {got:?}");
    if let Some(path) = std::env::var_os("RUSTCRAFT_S8_FISH")
        && let Ok(text) = std::fs::read_to_string(path)
    {
        let want: Vec<String> = text.lines().filter_map(|l| l.split_whitespace().nth(1).map(str::to_owned)).collect();
        assert_eq!(got, want);
    }
}

/// Fishing loot with a fixed seed (the world's `gameplay/fishing` sequence): 40 draws `loot … fish`,
/// item of each message; to compare with `S8/off_fish.txt` (`RUSTCRAFT_S8_FISH`).
#[test]
fn fishing_loot_sequence() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(dir) = prep_world("s8fish", 1, [200.5, -59.0, 200.5], &[]) else { return };
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    let (s, handle, _) = start_test(&dir, std::sync::Arc::new(data));
    std::thread::sleep(Duration::from_secs(2));
    let mut got = Vec::new();
    for _ in 0..40 {
        s.link
            .send_command("loot spawn 200 -50 200 fish minecraft:gameplay/fishing 200 -59 200 minecraft:fishing_rod")
            .unwrap();
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(3) {
            let Ok(e) = s.events.recv_timeout(Duration::from_millis(50)) else { continue };
            if let rc_client::Event::Chat { component, .. } = e {
                let txt = format!("{component:?}");
                if txt.contains("commands.drop.success") {
                    let rest = txt.split("item.minecraft.").nth(1).or_else(|| txt.split("block.minecraft.").nth(1));
                    let id =
                        rest.map(|r| r.split(|c: char| !(c.is_ascii_lowercase() || c == '_')).next().unwrap_or(""));
                    got.push(format!("minecraft:{}", id.unwrap_or("?")));
                    break;
                }
            }
        }
    }
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("fishing: {got:?}");
    if let Some(path) = std::env::var_os("RUSTCRAFT_S8_FISH")
        && let Ok(text) = std::fs::read_to_string(path)
    {
        let want: Vec<String> = text.lines().filter_map(|l| l.split_whitespace().nth(1).map(str::to_owned)).collect();
        assert_eq!(got, want);
    }
}

/// Reading of the first 60 ticks, written into `RUSTCRAFT_S8_OUT` (format of the official reading).
#[test]
fn trajectories_trace() {
    let Some(data) = rc_testdata::version_data() else { return };
    if std::env::var_os("RUSTCRAFT_S8_TRACE").is_none() {
        return;
    }
    let Some(dir) = prep_world("s8traj", 3, [205.5, -59.0, 208.5], &[]) else { return };
    inject(&dir, &scenario());
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    std::thread::sleep(Duration::from_secs(6));
    s.link.disconnect();
    handle.wait();
    let t = crate::projectiles::TRACE.lock().unwrap().clone();
    let mut out = String::new();
    for (uuid, _, tick, p, v) in &t {
        let n = (*uuid & 0xFFFF_FFFF) as i32;
        if *tick > 60 || (*uuid >> 96) as i32 != n {
            continue;
        }
        out.push_str(&format!("{n} {tick} {:?} {:?} {:?} {:?} {:?} {:?}\n", p[0], p[1], p[2], v[0], v[1], v[2]));
    }
    let all: Vec<Nbt> =
        (11..=13).flat_map(|cx| (12..=13).map(move |cz| (cx, cz))).flat_map(|(cx, cz)| saved(&dir, cx, cz)).collect();
    for e in &all {
        if e.get("id").and_then(Nbt::as_str) == Some("minecraft:pig") {
            out.push_str(&format!("pig_health {:?}\n", e.get("Health")));
        }
    }
    if let Some(path) = std::env::var_os("RUSTCRAFT_S8_OUT") {
        std::fs::write(path, &out).unwrap();
    }
    assert!(!t.is_empty(), "no projectile read");
    let _ = std::fs::remove_dir_all(&dir);
}
