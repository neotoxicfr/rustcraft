//! Mounts end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`): mounting and
//! taming a wild horse, saddled horse ridden then left, llama spit, camel that sits
//! down, save of a chest donkey and a rider.

use std::time::{Duration, Instant};

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::tests::{drain_until, inject, kind_id, mob_nbt, move_player, prep, saved};
use crate::tests::{start_test, test_world};

const X: f64 = 230.5;
const Z: f64 = 230.5;
const Y: f64 = -59.0;

fn stack(id: &str) -> Nbt {
    Nbt::Compound(vec![("id".into(), Nbt::String(format!("minecraft:{id}"))), ("count".into(), Nbt::Int(1))])
}

/// Network id of the first announced entity of this type.
fn wait_add(s: &rc_client::Session, kind: i32, secs: u64) -> Option<i32> {
    let mut id = None;
    drain_until(s, secs, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == kind
        {
            id = Some(a.id);
        }
        id.is_some()
    });
    id
}

/// Survival: mount a wild horse; it bucks (event 6) or lets itself be tamed (7); the player
/// remounts until it is tamed.
#[test]
fn wild_horse_bucks_then_tames() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f04tame", 0, "peaceful", &[]);
    move_player(&dir, [X, Y, Z]);
    inject(&dir, &[mob_nbt("horse", [X + 2.0, Y, Z], vec![])]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let id = wait_add(&s, kind_id(&data, "minecraft:horse"), 5).expect("horse announced");
    // Id of the singleplayer player.
    let player = 1;
    let (mut rides, mut bucks, mut tamed) = (0, 0, false);
    let t = Instant::now();
    while !tamed && t.elapsed() < Duration::from_secs(90) {
        s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
        let mut on = false;
        drain_until(&s, 2, |e| {
            if let Event::Entity(EntityEvent::Passengers { vehicle, passengers }) = e
                && *vehicle == id
                && passengers.contains(&player)
            {
                on = true;
            }
            on
        });
        if !on {
            continue;
        }
        rides += 1;
        drain_until(&s, 20, |e| match e {
            Event::Entity(EntityEvent::Event { id: i, event: 6 }) if *i == id => {
                bucks += 1;
                true
            }
            Event::Entity(EntityEvent::Event { id: i, event: 7 }) if *i == id => {
                tamed = true;
                true
            }
            _ => false,
        });
        // The server sends the player back to the ground after the buck: accept the teleportation.
        drain_until(&s, 1, |e| {
            if let Event::Teleport(p) = e {
                s.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).unwrap();
                return true;
            }
            false
        });
    }
    eprintln!("horse: {rides} mounts, {bucks} bucks, tamed {tamed}");
    assert!(rides >= 1, "the player does not mount");
    assert!(tamed, "never tamed");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tamed and saddled horse: the player pilots it (`move_vehicle`), the server follows, then
/// sneaking makes them dismount (teleportation next to it).
#[test]
fn saddled_horse_is_driven_then_left() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f04drive", 0, "peaceful", &[]);
    move_player(&dir, [X, Y, Z]);
    let horse = mob_nbt(
        "horse",
        [X + 2.0, Y, Z],
        vec![
            ("Tame".into(), Nbt::Byte(1)),
            ("equipment".into(), Nbt::Compound(vec![("saddle".into(), stack("saddle"))])),
        ],
    );
    inject(&dir, &[horse]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let id = wait_add(&s, kind_id(&data, "minecraft:horse"), 5).expect("horse announced");
    s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
    let mut tp = None;
    drain_until(&s, 3, |e| {
        if let Event::Teleport(p) = e {
            tp = Some(*p);
        }
        tp.is_some()
    });
    let tp = tp.expect("teleportation to the seat");
    s.link.send_accept_teleport(tp.id, tp.pos, tp.yaw, tp.pitch).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // The client advances the mount by 3 blocks east, in steps of 0.3.
    let mut x = X + 2.0;
    for _ in 0..10 {
        x += 0.3;
        s.link.send_move_vehicle([x, Y, Z], -90.0, 0.0, true).unwrap();
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut far = 0.0f64;
    drain_until(&s, 2, |e| {
        if let Event::Entity(EntityEvent::Sync(p)) = e
            && p.id == id
        {
            far = far.max(p.path.steps[0].0[0]);
        }
        if let Event::Entity(EntityEvent::Move(m)) = e
            && m.id == id
            && m.delta.is_some()
        {
            far = far.max(x);
        }
        far >= X + 4.5
    });
    eprintln!("horse ridden to x = {far:.2}");
    assert!(far >= X + 4.5, "the server does not follow the vehicle");
    s.link.send_input(32).unwrap();
    let off = drain_until(&s, 3, |e| matches!(e, Event::Teleport(_)));
    assert!(off, "no dismount on sneaking");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sneaking on a tamed chest donkey: mount screen with 5 columns, saddle placed by quick move
/// from the hotbar.
#[test]
fn donkey_inventory_opens() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f04menu", 0, "peaceful", &[("saddle", 1)]);
    move_player(&dir, [X, Y, Z]);
    let donkey =
        mob_nbt("donkey", [X + 2.0, Y, Z], vec![("Tame".into(), Nbt::Byte(1)), ("ChestedHorse".into(), Nbt::Byte(1))]);
    inject(&dir, &[donkey]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let id = wait_add(&s, kind_id(&data, "minecraft:donkey"), 5).expect("donkey announced");
    s.link.send_input(32).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(id, 0, [0.0, 0.5, 0.0], true).unwrap();
    let mut screen = None;
    drain_until(&s, 3, |e| {
        if let Event::MountScreen { window, columns, entity } = e {
            screen = Some((*window, *columns, *entity));
        }
        screen.is_some()
    });
    eprintln!("mount screen: {screen:?}");
    let (window, columns, entity) = screen.expect("no mount screen");
    assert_eq!((columns, entity), (5, id));
    // Hotbar slot 0 = menu slot 2 + 15 + 27; quick move to the saddle.
    s.link.send_container_click(window, 0, 2 + 15 + 27, 0, 1, &[], None).unwrap();
    let saddled = drain_until(
        &s,
        3,
        |e| matches!(e, Event::Entity(EntityEvent::Equipment { id: i, slots }) if *i == id && slots.iter().any(|(k, st)| *k == 7 && st.is_some())),
    );
    assert!(saddled, "saddle not placed by the menu");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Demo world (`RUSTCRAFT_F04_PREP=<folder>`): player on a saddled and armored horse
/// (`RootVehicle`), looking south; the nine species in an arc in front of them
/// (`RUSTCRAFT_F04_AI`: AI active).
#[test]
#[ignore]
fn prep_f04_world() {
    let Ok(dir) = std::env::var("RUSTCRAFT_F04_PREP") else { return };
    let Some(world) = test_world() else { return };
    let dir = std::path::PathBuf::from(dir);
    let made = prep(&world, "f04prep", 1, "normal", &[("saddle", 1), ("wheat", 64), ("chest", 1)]);
    let (x, z) = (X, Z);
    let uuid = |n: u128| crate::nbtio::uuid_nbt(0x0F04_0000_0000_4000_8000_0000_0000_0000 | n);
    let horse = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:horse".into())),
        ("Pos".into(), Nbt::List([x, Y, z].iter().map(|&v| Nbt::Double(v)).collect())),
        ("Rotation".into(), Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(0.0)])),
        ("UUID".into(), uuid(1)),
        ("Tame".into(), Nbt::Byte(1)),
        ("Variant".into(), Nbt::Int(3 | 2 << 8)),
        ("PersistenceRequired".into(), Nbt::Byte(1)),
        (
            "equipment".into(),
            Nbt::Compound(vec![("saddle".into(), stack("saddle")), ("body".into(), stack("diamond_horse_armor"))]),
        ),
    ]);
    for e in std::fs::read_dir(made.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).unwrap();
        crate::nbtio::set(&mut pl, "Pos", Nbt::List([x, Y + 1.4, z].iter().map(|&v| Nbt::Double(v)).collect()));
        crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(12.0)]));
        let rv = Nbt::Compound(vec![("Attach".into(), uuid(1)), ("Entity".into(), horse.clone())]);
        crate::nbtio::set(&mut pl, "RootVehicle", rv);
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
    }
    let ai = |extra: Vec<(String, Nbt)>| {
        let mut v = vec![
            ("NoAI".to_owned(), Nbt::Byte(i8::from(std::env::var_os("RUSTCRAFT_F04_AI").is_none()))),
            ("Rotation".to_owned(), Nbt::List(vec![Nbt::Float(150.0), Nbt::Float(0.0)])),
        ];
        v.extend(extra);
        v
    };
    let rider = |id: &str, hand: Option<&str>| {
        let mut f = vec![
            ("id".to_owned(), Nbt::String(format!("minecraft:{id}"))),
            ("Pos".to_owned(), Nbt::List([x, Y, z].iter().map(|&v| Nbt::Double(v)).collect())),
            ("PersistenceRequired".to_owned(), Nbt::Byte(1)),
        ];
        if let Some(h) = hand {
            f.push(("equipment".into(), Nbt::Compound(vec![("mainhand".into(), stack(h))])));
        }
        Nbt::Compound(f)
    };
    let carpet = Nbt::Compound(vec![("body".into(), stack("red_carpet"))]);
    let saddle = Nbt::Compound(vec![("saddle".into(), stack("saddle"))]);
    inject(
        &made,
        &[
            mob_nbt(
                "donkey",
                [x - 6.0, Y, z + 6.0],
                ai(vec![("ChestedHorse".into(), Nbt::Byte(1)), ("Tame".into(), Nbt::Byte(1))]),
            ),
            mob_nbt("mule", [x - 3.5, Y, z + 8.0], ai(Vec::new())),
            mob_nbt(
                "llama",
                [x - 1.0, Y, z + 9.0],
                ai(vec![
                    ("Variant".into(), Nbt::Int(2)),
                    ("Strength".into(), Nbt::Int(3)),
                    ("equipment".into(), carpet),
                ]),
            ),
            mob_nbt("trader_llama", [x + 1.5, Y, z + 9.5], ai(vec![("Variant".into(), Nbt::Int(1))])),
            mob_nbt(
                "skeleton_horse",
                [x + 4.0, Y, z + 8.5],
                ai(vec![("Passengers".into(), Nbt::List(vec![rider("skeleton", Some("bow"))]))]),
            ),
            mob_nbt(
                "zombie_horse",
                [x + 6.5, Y, z + 6.5],
                ai(vec![("Passengers".into(), Nbt::List(vec![rider("zombie", Some("iron_spear"))]))]),
            ),
            mob_nbt(
                "camel",
                [x - 7.0, Y, z + 12.5],
                ai(vec![("LastPoseTick".into(), Nbt::Long(-1)), ("equipment".into(), saddle)]),
            ),
            mob_nbt(
                "camel_husk",
                [x + 3.0, Y, z + 14.0],
                ai(vec![(
                    "Passengers".into(),
                    Nbt::List(vec![rider("husk", Some("iron_spear")), rider("parched", Some("bow"))]),
                )]),
            ),
            mob_nbt(
                "horse",
                [x - 2.0, Y, z + 4.5],
                ai(vec![("Age".into(), Nbt::Int(-24000)), ("Variant".into(), Nbt::Int(1))]),
            ),
        ],
    );
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&made, &dir).unwrap();
    eprintln!("world prepared: {}", dir.display());
}

/// Survival: a hit llama spits (`llama_spit` entity) at the player.
#[test]
fn llama_spits_back() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f04spit", 0, "normal", &[]);
    move_player(&dir, [X, Y, Z]);
    inject(&dir, &[mob_nbt("llama", [X + 4.0, Y, Z], vec![])]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let id = wait_add(&s, kind_id(&data, "minecraft:llama"), 5).expect("llama announced");
    std::thread::sleep(Duration::from_millis(500));
    s.link.send_attack(id).unwrap();
    let spit = wait_add(&s, kind_id(&data, "minecraft:llama_spit"), 10);
    eprintln!("spit: {spit:?}");
    assert!(spit.is_some(), "the llama does not spit");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A camel rested for a long time ends up sitting down (posture 10).
#[test]
fn camel_sits_down() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f04camel", 0, "peaceful", &[]);
    move_player(&dir, [X, Y, Z]);
    inject(&dir, &[mob_nbt("camel", [X + 4.0, Y, Z + 4.0], vec![])]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let id = wait_add(&s, kind_id(&data, "minecraft:camel"), 5).expect("camel announced");
    let mut sat = false;
    let t0 = std::time::Instant::now();
    drain_until(&s, 60, |e| {
        if let Event::Entity(EntityEvent::Data { id: i, values }) = e
            && *i == id
            && values.iter().any(|(k, v)| *k == 6 && *v == Meta::Pose(10))
        {
            sat = true;
        }
        sat
    });
    eprintln!("camel sitting: {sat} after {:?}", t0.elapsed());
    assert!(sat, "the camel does not sit down");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save: chest donkey (items), zombie horse and its rider (`Passengers`).
#[test]
fn saves_chest_and_rider() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f04save", 1, "normal", &[]);
    move_player(&dir, [X, Y, Z]);
    let mut apple = stack("apple");
    crate::nbtio::set(&mut apple, "Slot", Nbt::Byte(3));
    let donkey = mob_nbt(
        "donkey",
        [X + 3.0, Y, Z],
        vec![
            ("Tame".into(), Nbt::Byte(1)),
            ("ChestedHorse".into(), Nbt::Byte(1)),
            ("Items".into(), Nbt::List(vec![apple])),
        ],
    );
    // Tamed mount, like those of the skeleton trap: a wild mount that is ridden throws its rider off
    // at random (mad dash, one chance in 50 per tick, like the game), which
    // made this test intermittent.
    let rider = mob_nbt("skeleton", [X - 3.0, Y + 1.0, Z], vec![]);
    let horse = mob_nbt(
        "skeleton_horse",
        [X - 3.0, Y, Z],
        vec![("Tame".into(), Nbt::Byte(1)), ("Passengers".into(), Nbt::List(vec![rider]))],
    );
    inject(&dir, &[donkey, horse]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let sk = kind_id(&data, "minecraft:skeleton_horse");
    let mut linked = false;
    let mut horse_id = None;
    drain_until(&s, 5, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == sk => horse_id = Some(a.id),
            Event::Entity(EntityEvent::Passengers { vehicle, passengers })
                if Some(*vehicle) == horse_id && !passengers.is_empty() =>
            {
                linked = true
            }
            _ => {}
        }
        linked
    });
    assert!(linked, "rider not announced on its mount");
    std::thread::sleep(Duration::from_millis(500));
    s.link.disconnect();
    handle.wait();
    let list = saved(&dir, (X as i32) >> 4, (Z as i32) >> 4);
    let d = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:donkey")).expect("donkey");
    assert_eq!(crate::nbtio::int(d, "ChestedHorse"), Some(1));
    assert_eq!(crate::nbtio::list(d.get("Items")).len(), 1);
    let h = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:skeleton_horse")).expect("horse");
    let ps = crate::nbtio::list(h.get("Passengers"));
    assert_eq!(ps.len(), 1, "rider saved in its mount");
    assert!(!list.iter().any(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:skeleton")));
    let _ = std::fs::remove_dir_all(&dir);
}
