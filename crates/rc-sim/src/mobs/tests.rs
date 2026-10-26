//! Mobs end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, flat map):
//! spawn eggs, wandering, player hits and loot, zombie that hits, creeper that
//! explodes, skeleton that shoots, save. Absent: tests ignored.

use std::time::{Duration, Instant};

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use crate::tests::{fresh_copy, start_test, test_world};

/// Copy of the world: game mode, difficulty, inventory (hotbar: `items`), full health.
pub(super) fn prep(
    world: &std::path::Path,
    tag: &str,
    game_type: i32,
    difficulty: &str,
    items: &[(&str, i32)],
) -> std::path::PathBuf {
    let dir = fresh_copy(world, tag);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        let mut ds = d.get("difficulty_settings").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        crate::nbtio::set(&mut ds, "difficulty", Nbt::String(difficulty.into()));
        crate::nbtio::set(d, "difficulty_settings", ds);
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    // Mob loot (disabled on the test map).
    let gp = dir.join("data").join("minecraft").join("game_rules.dat");
    let mut g = crate::nbtio::read_dat(&gp).unwrap_or(Nbt::Compound(Vec::new()));
    let mut d = g.get("data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
    crate::nbtio::set(&mut d, "minecraft:mob_drops", Nbt::Byte(1));
    crate::nbtio::set(&mut g, "data", d);
    std::fs::write(&gp, crate::nbtio::gzip(&crate::nbtio::encode_root(&g))).unwrap();
    let inv: Vec<Nbt> = items
        .iter()
        .enumerate()
        .map(|(i, (id, n))| {
            Nbt::Compound(vec![
                ("Slot".into(), Nbt::Byte(i as i8)),
                ("id".into(), Nbt::String(format!("minecraft:{id}"))),
                ("count".into(), Nbt::Int(*n)),
            ])
        })
        .collect();
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).unwrap();
        crate::nbtio::set(&mut pl, "playerGameType", Nbt::Int(game_type));
        crate::nbtio::set(&mut pl, "Inventory", Nbt::List(inv.clone()));
        crate::nbtio::set(&mut pl, "Health", Nbt::Float(20.0));
        crate::nbtio::set(&mut pl, "foodLevel", Nbt::Int(20));
        crate::nbtio::set(&mut pl, "SelectedItemSlot", Nbt::Int(0));
        crate::nbtio::remove(&mut pl, "equipment");
        let mut ab = pl.get("abilities").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for k in ["flying", "invulnerable"] {
            crate::nbtio::set(&mut ab, k, Nbt::Byte(0));
        }
        crate::nbtio::set(&mut ab, "instabuild", Nbt::Byte(i8::from(game_type == 1)));
        crate::nbtio::set(&mut ab, "mayfly", Nbt::Byte(i8::from(game_type == 1)));
        crate::nbtio::set(&mut pl, "abilities", ab);
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
    }
    dir
}

/// Preparation of a demonstration world (`RUSTCRAFT_S4_PREP=<world folder>`,
/// `RUSTCRAFT_S4_SCENARIO=eggs|fight`): eggs in the hotbar, mode and difficulty.
#[test]
#[ignore]
fn prep_mobs_world() {
    let (Ok(dir), Ok(sc)) = (std::env::var("RUSTCRAFT_S4_PREP"), std::env::var("RUSTCRAFT_S4_SCENARIO")) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let tmp = dir.with_extension("src");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::rename(&dir, &tmp).unwrap();
    let (mode, items): (i32, Vec<(&str, i32)>) = match sc.as_str() {
        "mine" => (0, vec![("iron_pickaxe", 1), ("stone", 16)]),
        // F03: cubes placed in front of the player (screenshots).
        s if s.starts_with("cubes") => (
            if s == "cubes" { 1 } else { 0 },
            vec![("slime_spawn_egg", 8), ("magma_cube_spawn_egg", 8), ("sulfur_cube_spawn_egg", 8), ("tnt", 4)],
        ),
        "fight" => (
            0,
            vec![
                ("diamond_sword", 1),
                ("zombie_spawn_egg", 4),
                ("skeleton_spawn_egg", 4),
                ("creeper_spawn_egg", 4),
                ("spider_spawn_egg", 4),
                ("cow_spawn_egg", 4),
            ],
        ),
        _ => (
            1,
            vec![
                ("cow_spawn_egg", 8),
                ("pig_spawn_egg", 8),
                ("sheep_spawn_egg", 8),
                ("chicken_spawn_egg", 8),
                ("wolf_spawn_egg", 8),
                ("cat_spawn_egg", 8),
                ("villager_spawn_egg", 8),
                ("zombie_spawn_egg", 8),
                ("skeleton_spawn_egg", 8),
            ],
        ),
    };
    let made = prep(&tmp, "s4prep", mode, "normal", &items);
    if sc.starts_with("cubes") {
        cubes_scene(&made, &sc);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::rename(&made, &dir).unwrap();
    eprintln!("world prepared: {}", dir.display());
}

/// Player moved (files of all the players).
pub(super) fn move_player(dir: &std::path::Path, pos: [f64; 3]) {
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).unwrap();
        crate::nbtio::set(&mut pl, "Pos", Nbt::List(pos.iter().map(|&v| Nbt::Double(v)).collect()));
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
    }
}

pub(super) fn kind_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:entity_type").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

/// Places an egg (hotbar slot 0, main hand) on the ground `dx` blocks east of the player;
/// returns the identifier of the announced mob.
pub(super) fn spawn_egg(
    s: &rc_client::Session,
    data: &rc_data::VersionData,
    p: [f64; 3],
    dx: i32,
    slot: i16,
    kind: &str,
    seq: i32,
) -> i32 {
    s.link.send_carried_item(slot).unwrap();
    let ground = [p[0].floor() as i32 + dx, p[1].floor() as i32 - 1, p[2].floor() as i32];
    s.link.send_use_item_on(ground, 1, [0.5, 1.0, 0.5], false, seq).unwrap();
    let k = kind_id(data, kind);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        if let Ok(Event::Entity(EntityEvent::Add(a))) = s.events.recv_timeout(Duration::from_millis(50))
            && a.kind == k
        {
            return a.id;
        }
    }
    panic!("{kind} did not appear");
}

pub(super) fn drain_until(s: &rc_client::Session, secs: u64, mut f: impl FnMut(&Event) -> bool) -> bool {
    crate::tests::wait_game(s, secs, &mut f)
}

/// Creative: cow from an egg, it wanders (relative moves received), then save and re-read.
#[test]
fn cow_egg_wander_and_save() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4cow", 1, "normal", &[("cow_spawn_egg", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let id = spawn_egg(&s, &data, p.pos, 3, 0, "minecraft:cow", 2);
    let mut moves = 0;
    drain_until(&s, 25, |e| {
        if let Event::Entity(EntityEvent::Move(m)) = e
            && m.id == id
            && m.delta.is_some()
        {
            moves += 1;
        }
        moves >= 5
    });
    eprintln!("cow {id}: {moves} moves");
    assert!(moves >= 5, "the cow does not wander ({moves})");
    s.link.disconnect();
    handle.wait();
    // Re-read: the cow is announced again.
    let (s, handle, _) = start_test(&dir, data.clone());
    let k = kind_id(&data, "minecraft:cow");
    let back = drain_until(&s, 10, |e| matches!(e, Event::Entity(EntityEvent::Add(a)) if a.kind == k));
    assert!(back, "cow not read back");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: the hit cow dies (event 3) and drops its loot and experience.
#[test]
fn player_kills_cow_loot() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4kill", 0, "normal", &[("cow_spawn_egg", 1), ("diamond_sword", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let id = spawn_egg(&s, &data, p.pos, 2, 0, "minecraft:cow", 2);
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    let (item, orb) = (kind_id(&data, "minecraft:item"), kind_id(&data, "minecraft:experience_orb"));
    let (mut dead, mut hurts, mut items, mut orbs) = (false, 0, 0, 0);
    for _ in 0..8 {
        s.link.send_attack(id).unwrap();
        s.link.send_punch().unwrap();
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(900) {
            let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
            match e {
                Event::Entity(EntityEvent::Hurt { id: i, .. }) if i == id => hurts += 1,
                Event::Entity(EntityEvent::Event { id: i, event: 3 }) if i == id => dead = true,
                Event::Entity(EntityEvent::Add(a)) if a.kind == item => items += 1,
                Event::Entity(EntityEvent::Add(a)) if a.kind == orb => orbs += 1,
                _ => {}
            }
        }
        if dead && items > 0 {
            break;
        }
    }
    eprintln!("hits {hurts}, dead {dead}, items {items}, orbs {orbs}");
    assert!(dead, "cow not killed");
    assert!(items > 0 || orbs > 0, "no loot");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival, normal: the zombie placed 3 blocks away comes to hit the player (health goes down).
#[test]
fn zombie_hits_player() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4zombie", 0, "normal", &[("zombie_spawn_egg", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let _ = spawn_egg(&s, &data, p.pos, 3, 0, "minecraft:zombie", 2);
    let hit = drain_until(&s, 15, |e| matches!(e, Event::Health { health, .. } if *health < 20.0));
    assert!(hit, "the zombie did not hit");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: the creeper placed 2 blocks away swells and explodes.
#[test]
fn creeper_explodes() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4creeper", 0, "normal", &[("creeper_spawn_egg", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let _ = spawn_egg(&s, &data, p.pos, 2, 0, "minecraft:creeper", 2);
    let boom = drain_until(&s, 15, |e| matches!(e, Event::Explode(_)));
    assert!(boom, "no explosion");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: the skeleton placed 8 blocks away shoots arrows.
#[test]
fn skeleton_shoots() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4skel", 0, "normal", &[("skeleton_spawn_egg", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let skel = spawn_egg(&s, &data, p.pos, 8, 0, "minecraft:skeleton", 2);
    let arrow = kind_id(&data, "minecraft:arrow");
    let mut owner = None;
    let shot = drain_until(&s, 15, |e| match e {
        Event::Entity(EntityEvent::Add(a)) if a.kind == arrow => {
            owner = Some(a.data);
            true
        }
        _ => false,
    });
    assert!(shot, "no arrow");
    // Spawn data of a projectile: shooter identifier as is (no +1).
    assert_eq!(owner, Some(skel), "shooter announced");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- Numeric comparisons with the official server ----------

/// Entities added to the entity chunks of the world (main dimension).
pub(super) fn inject(dir: &std::path::Path, ents: &[Nbt]) {
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
pub(super) fn saved(dir: &std::path::Path, cx: i32, cz: i32) -> Vec<Nbt> {
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

pub(super) fn mob_nbt(id: &str, pos: [f64; 3], extra: Vec<(String, Nbt)>) -> Nbt {
    let mut f = vec![
        ("id".to_owned(), Nbt::String(format!("minecraft:{id}"))),
        ("Pos".to_owned(), Nbt::List(pos.iter().map(|&v| Nbt::Double(v)).collect())),
        ("PersistenceRequired".to_owned(), Nbt::Byte(1)),
    ];
    f.extend(extra);
    Nbt::Compound(f)
}

fn health_of(list: &[Nbt], id: &str) -> Vec<f32> {
    list.iter()
        .filter(|e| e.get("id").and_then(Nbt::as_str) == Some(id))
        .filter_map(|e| crate::nbtio::float(e, "Health"))
        .collect()
}

/// Fall of 10 blocks (cow, immune chicken, zombie) and explosion of a creeper lit 3 blocks
/// from a zombie without AI: healths read back after saving. Figures of the official server (same
/// scenario, `S4-notes.md`): cow 3, chicken 4, zombie 13; zombie blown away: see notes.
#[test]
fn parity_fall_and_blast() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4parity", 1, "normal", &[]);
    // Clear area of the flat map (same scenario as on the official server).
    let (x, z) = (210.5, 210.5);
    move_player(&dir, [x + 4.0, -59.0, z - 4.0]);
    let none = || Vec::new();
    inject(
        &dir,
        &[
            mob_nbt("cow", [x, -49.0, z], none()),
            mob_nbt("chicken", [x + 2.0, -49.0, z], none()),
            mob_nbt("pig", [x + 8.0, -59.0, z], vec![("NoAI".into(), Nbt::Byte(1))]),
            mob_nbt("creeper", [x + 8.0, -59.0, z + 5.0], vec![("ignited".into(), Nbt::Byte(1))]),
        ],
    );
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    std::thread::sleep(Duration::from_secs(4));
    s.link.disconnect();
    handle.wait();
    let all: Vec<Nbt> =
        (12..=14).flat_map(|cx| (12..=14).map(move |cz| (cx, cz))).flat_map(|(cx, cz)| saved(&dir, cx, cz)).collect();
    eprintln!(
        "cow {:?}, chicken {:?}, pig {:?}",
        health_of(&all, "minecraft:cow"),
        health_of(&all, "minecraft:chicken"),
        health_of(&all, "minecraft:pig")
    );
    assert_eq!(health_of(&all, "minecraft:cow"), vec![3.0]);
    assert_eq!(health_of(&all, "minecraft:chicken"), vec![4.0]);
    // Official server: 4.9166665 (seen part 1, damage 5.0833335).
    assert_eq!(health_of(&all, "minecraft:pig"), vec![4.916_666_5]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: breaking grass with a pickaxe → the held slot is sent back with the wear component,
/// and the item on the ground is announced with its stack (metadata 8).
#[test]
fn pickaxe_wear_and_drop_sent() {
    use rc_proto_v777::item::Inventory;
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4wear", 0, "normal", &[("iron_pickaxe", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    s.link.send_carried_item(0).unwrap();
    // Player on the ground (otherwise mining speed / 5) and chunks loaded before the hit.
    s.link.send_move(rc_client::link::Move::Pos { pos: p.pos }, true, false).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    let ground = [p.pos[0].floor() as i32 + 1, p.pos[1].floor() as i32 - 1, p.pos[2].floor() as i32];
    s.link.send_player_action(0, ground, 1, 3).unwrap();
    // Grass with a pickaxe: 18 ticks. End sent after 70 % (otherwise deferred break, which the game never
    // does for a recently connected player: see `delayed_destroy_uses_the_start_tick`).
    std::thread::sleep(Duration::from_millis(1500));
    s.link.send_player_action(3, ground, 1, 4).unwrap();
    let item = kind_id(&data, "minecraft:item");
    let (mut wear, mut item_meta, mut item_add) = (None, false, None);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(4) {
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        match e {
            Event::Inventory(Inventory::Slot { slot: 36, stack: Some(st), .. }) => wear = Some(st.added.len()),
            Event::Entity(EntityEvent::Add(a)) if a.kind == item => item_add = Some(a.id),
            Event::Entity(EntityEvent::Data { id, values }) if Some(id) == item_add => {
                item_meta =
                    values.iter().any(|(i, v)| *i == 8 && matches!(v, rc_proto_v777::entity::Meta::Item(Some(_))));
            }
            _ => {}
        }
    }
    eprintln!("wear: components {wear:?}, item {item_add:?} stack {item_meta}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(wear.is_some_and(|n| n > 0), "wear not sent");
    assert!(item_meta, "stack of the item on the ground not sent");
}

/// Packet trace of a thrown item (debugging the display of items on the ground).
#[test]
#[ignore]
fn trace_dropped_item() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "s4trace", 0, "normal", &[("iron_pickaxe", 1), ("stone", 16)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_player_action(5, [0, 0, 0], 0, 0).unwrap();
    let item = kind_id(&data, "minecraft:item");
    let mut id = None;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        let Ok(Event::Entity(e)) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        let mine = match &e {
            EntityEvent::Add(a) if a.kind == item => {
                id = Some(a.id);
                true
            }
            EntityEvent::Data { id: i, .. } | EntityEvent::Motion { id: i, .. } => Some(*i) == id,
            EntityEvent::Sync(p) => Some(p.id) == id,
            EntityEvent::Move(m) => Some(m.id) == id,
            EntityEvent::Remove(l) => id.is_some_and(|i| l.contains(&i)),
            _ => false,
        };
        if mine {
            eprintln!("{:.2} {e:?}", t.elapsed().as_secs_f32());
        }
    }
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- F03: slimes and cubes ----------

/// Test world with the player at (210.5, −59, 210.5) and injected cubes; ids of the mobs
/// announced of a species.
fn cube_start(
    tag: &str,
    mode: i32,
    items: &[(&str, i32)],
    ents: &[Nbt],
) -> (std::path::PathBuf, rc_client::Session, crate::Handle, std::sync::Arc<rc_data::VersionData>) {
    let (world, data) = (test_world().unwrap(), rc_testdata::version_data().unwrap());
    let dir = prep(&world, tag, mode, "normal", items);
    move_player(&dir, [210.5, -59.0, 210.5]);
    // Looking east (+x), horizontal.
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("dat") {
            let mut pl = crate::nbtio::read_dat(&p).unwrap();
            crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(-90.0), Nbt::Float(0.0)]));
            std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
        }
    }
    inject(&dir, ents);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    (dir, s, handle, data)
}

fn first_of(s: &rc_client::Session, kind: i32, secs: u64) -> Option<i32> {
    let mut id = None;
    drain_until(s, secs, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == kind
        {
            id = Some(a.id);
            return true;
        }
        false
    });
    id
}

fn first_health(s: &rc_client::Session, secs: u64) -> Option<f32> {
    let mut h = None;
    drain_until(s, secs, |e| {
        if let Event::Health { health, .. } = e
            && *health < 20.0
        {
            h = Some(*health);
            return true;
        }
        false
    });
    h
}

/// Survival, normal: a size-2 slime in contact hits for 2 (health 18); a size-2 magma cube
/// hits for 2 + 2 (health 16).
#[test]
fn cubes_contact_damage_by_size() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    for (id, want) in [("slime", 18.0), ("magma_cube", 16.0)] {
        // Without AI, a cube does no contact damage: AI kept.
        let e = mob_nbt(id, [211.8, -59.0, 210.5], vec![("Size".into(), Nbt::Int(1))]);
        let (dir, s, handle, _) = cube_start(&format!("f03{id}"), 0, &[], &[e]);
        let h = first_health(&s, 10);
        eprintln!("{id}: health {h:?}");
        s.link.disconnect();
        handle.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(h, Some(want), "{id}");
    }
}

/// Size-2 slime killed: two to four size-1 slimes are born at removal (20 ticks).
#[test]
fn slime_splits_on_death() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let e = mob_nbt(
        "slime",
        [212.5, -59.0, 210.5],
        vec![("Size".into(), Nbt::Int(1)), ("Health".into(), Nbt::Float(1.0)), ("NoAI".into(), Nbt::Byte(1))],
    );
    let (dir, s, handle, data) = cube_start("f03split", 1, &[("diamond_sword", 1)], &[e]);
    let slime = kind_id(&data, "minecraft:slime");
    let id = first_of(&s, slime, 10).expect("slime announced");
    std::thread::sleep(Duration::from_millis(500));
    s.link.send_attack(id).unwrap();
    let mut dead = false;
    let mut born = 0;
    drain_until(&s, 4, |e| {
        match e {
            Event::Entity(EntityEvent::Event { id: i, event: 3 }) if *i == id => dead = true,
            Event::Entity(EntityEvent::Add(a)) if a.kind == slime => born += 1,
            _ => {}
        }
        false
    });
    eprintln!("dead {dead}, small {born}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(dead, "slime not killed");
    assert!((2..=4).contains(&born), "split: {born}");
}

/// Sulfur cube: absorbs the TNT held out to it ("body" equipment), gives it back when sheared, then
/// goes into the bucket.
#[test]
fn sulfur_swallow_shear_bucket() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let e = mob_nbt(
        "sulfur_cube",
        [212.5, -59.0, 210.5],
        vec![("Size".into(), Nbt::Int(1)), ("NoAI".into(), Nbt::Byte(1))],
    );
    let (dir, s, handle, data) = cube_start("f03sulfur", 0, &[("tnt", 2), ("shears", 1), ("bucket", 1)], &[e]);
    let cube = kind_id(&data, "minecraft:sulfur_cube");
    let item = kind_id(&data, "minecraft:item");
    let id = first_of(&s, cube, 10).expect("cube announced");
    std::thread::sleep(Duration::from_millis(300));
    let body = |s: &rc_client::Session| {
        let mut got = None;
        drain_until(s, 3, |e| {
            if let Event::Entity(EntityEvent::Equipment { id: i, slots }) = e
                && *i == id
                && let Some((_, st)) = slots.iter().find(|(k, _)| *k == 6)
            {
                got = Some(st.is_some());
                return true;
            }
            false
        });
        got
    };
    s.link.send_carried_item(0).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(id, 0, [0.0, 0.25, 0.0], false).unwrap();
    let swallowed = body(&s);
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(id, 0, [0.0, 0.25, 0.0], false).unwrap();
    let mut dropped = false;
    let mut emptied = None;
    drain_until(&s, 3, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == item => dropped = true,
            Event::Entity(EntityEvent::Equipment { id: i, slots }) if *i == id => {
                emptied = slots.iter().find(|(k, _)| *k == 6).map(|(_, st)| st.is_none());
            }
            _ => {}
        }
        dropped && emptied.is_some()
    });
    s.link.send_carried_item(2).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(id, 0, [0.0, 0.25, 0.0], false).unwrap();
    let gone = drain_until(&s, 3, |e| matches!(e, Event::Entity(EntityEvent::Remove(l)) if l.contains(&id)));
    eprintln!("absorbed {swallowed:?}, dropped {dropped}, emptied {emptied:?}, bucket {gone}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(swallowed, Some(true), "TNT not absorbed");
    assert!(dropped && emptied == Some(true), "shearing");
    assert!(gone, "bucket");
}

/// Full sulfur cube hit bare-handed: no damage (no "hurt"), it is thrown.
#[test]
fn sulfur_punch_bounces() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let tnt = Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:tnt".into())), ("count".into(), Nbt::Int(1))]);
    let e = mob_nbt(
        "sulfur_cube",
        [212.0, -59.0, 210.5],
        vec![("Size".into(), Nbt::Int(1)), ("equipment".into(), Nbt::Compound(vec![("body".into(), tnt)]))],
    );
    let (dir, s, handle, data) = cube_start("f03punch", 0, &[], &[e]);
    let cube = kind_id(&data, "minecraft:sulfur_cube");
    let id = first_of(&s, cube, 10).expect("cube announced");
    std::thread::sleep(Duration::from_millis(600));
    s.link.send_attack(id).unwrap();
    let (mut hurt, mut moved) = (false, None);
    drain_until(&s, 3, |e| {
        match e {
            Event::Entity(EntityEvent::Hurt { id: i, .. }) if *i == id => hurt = true,
            Event::Entity(EntityEvent::Motion { id: i, velocity }) if *i == id && moved.is_none() => {
                moved = Some(*velocity)
            }
            _ => {}
        }
        false
    });
    eprintln!("hurt {hurt}, speed {moved:?}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!hurt, "the full cube must not be hurt");
    let v = moved.expect("no knockback");
    assert!(v[0] > 0.1, "knockback towards +x expected: {v:?}");
}

/// F03 capture scene: player at (210.5, −59, 210.5) turned east, cubes of all
/// sizes in front of them (`cubes`), or a single full sulfur cube to punch (`cubes_punch`).
fn cubes_scene(dir: &std::path::Path, sc: &str) {
    move_player(dir, [210.5, -59.0, 210.5]);
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("dat") {
            let mut pl = crate::nbtio::read_dat(&p).unwrap();
            crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(-90.0), Nbt::Float(15.0)]));
            std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
        }
    }
    let size = |n: i32| ("Size".to_owned(), Nbt::Int(n));
    let body = |id: &str| {
        let s =
            Nbt::Compound(vec![("id".into(), Nbt::String(format!("minecraft:{id}"))), ("count".into(), Nbt::Int(1))]);
        ("equipment".to_owned(), Nbt::Compound(vec![("body".into(), s)]))
    };
    let ents = if sc == "cubes_punch" {
        vec![mob_nbt("sulfur_cube", [212.3, -59.0, 210.5], vec![size(1), body("tnt")])]
    } else {
        vec![
            mob_nbt("slime", [214.5, -59.0, 207.5], vec![size(0)]),
            mob_nbt("slime", [216.0, -59.0, 209.0], vec![size(1)]),
            mob_nbt("slime", [220.0, -59.0, 212.0], vec![size(3)]),
            mob_nbt("magma_cube", [216.0, -59.0, 213.5], vec![size(1)]),
            mob_nbt("magma_cube", [214.0, -59.0, 215.0], vec![size(0)]),
            mob_nbt("sulfur_cube", [213.5, -59.0, 210.5], vec![size(1), body("tnt")]),
            mob_nbt("sulfur_cube", [214.0, -59.0, 212.0], vec![size(1), body("grass_block")]),
            mob_nbt("sulfur_cube", [213.0, -59.0, 208.5], vec![size(0), ("Age".into(), Nbt::Int(-24000))]),
        ]
    };
    inject(dir, &ents);
}
