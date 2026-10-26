//! Family F02 end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, flat map):
//! eggs, rabbit leaps, bat taking flight, parrot tamed then killed by cookie, mooshroom
//! (stew, shearing into a cow), bear that rears up and strikes, save.

use std::time::{Duration, Instant};

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::tests::{drain_until, kind_id, prep, saved, spawn_egg};
use crate::tests::{start_test, test_world};

fn item_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:item").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

/// Creative: the rabbit leaps (event 1) and moves; the bat takes flight
/// (level event 1025, rest flag cleared) and flies.
#[test]
fn rabbit_hops_bat_flies() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f02hop", 1, "normal", &[("rabbit_spawn_egg", 1), ("bat_spawn_egg", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    // Chunks loaded before aiming at the ground.
    std::thread::sleep(Duration::from_millis(1500));
    let rabbit = spawn_egg(&s, &data, p.pos, 3, 0, "minecraft:rabbit", 2);
    let bat = spawn_egg(&s, &data, p.pos, -3, 1, "minecraft:bat", 3);
    let (mut hops, mut rmoves, mut woke, mut bat_awake, mut bmoves) = (0, 0, false, false, 0);
    // The rabbit leaps only while strolling, start drawn at random (about one chance in 120
    // per tick): in 20 s, a single stroll sometimes occurred. A minute of play gives about
    // ten on average; the wait stops once three leaps are seen.
    drain_until(&s, 60, |e| {
        match e {
            Event::Entity(EntityEvent::Event { id, event: 1 }) if *id == rabbit => hops += 1,
            Event::Entity(EntityEvent::Move(m)) if m.id == rabbit && m.delta.is_some() => rmoves += 1,
            Event::Entity(EntityEvent::Move(m)) if m.id == bat && m.delta.is_some() => bmoves += 1,
            Event::LevelEvent { kind: 1025, .. } => woke = true,
            Event::Entity(EntityEvent::Data { id, values })
                if *id == bat && values.iter().any(|(i, v)| *i == 16 && *v == Meta::Byte(0)) =>
            {
                bat_awake = true;
            }
            _ => {}
        }
        hops >= 3 && rmoves >= 3 && bmoves >= 10
    });
    eprintln!("rabbit: {hops} leaps, {rmoves} moves; bat: 1025 {woke}, wake {bat_awake}, {bmoves} moves");
    assert!(hops >= 3 && rmoves >= 3, "the rabbit does not leap");
    // Wake (1025, flag 16) on the first tick, before the announcement: only flight is checked.
    assert!(bmoves >= 10, "the bat does not fly");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Copy of the world with commands allowed (glass cells placed by `fill`).
fn allow_commands(dir: &std::path::Path) {
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
}

/// Waits for the chunk containing `(x, z)` to be sent: before that, the server considers it unloaded and
/// refuses `fill` ("argument.pos.unloaded"), like the game just after joining.
fn wait_chunk(s: &rc_client::Session, x: i32, z: i32) {
    let got = drain_until(s, 10, |e| matches!(e, Event::Chunk(c) if (c.x, c.z) == (x >> 4, z >> 4)));
    assert!(got, "chunk of the cell never received");
}

/// Glass cell of every other block around `(x, y, z)` (four walls and a roof):
/// a parrot locked in no longer wanders out of range, whatever its random.
fn glass_cell(x: i32, y: i32, z: i32) -> Vec<String> {
    let g = "minecraft:glass";
    vec![
        format!("fill {} {y} {} {} {} {} {g}", x - 1, z - 1, x + 1, y + 1, z - 1),
        format!("fill {} {y} {} {} {} {} {g}", x - 1, z + 1, x + 1, y + 1, z + 1),
        format!("fill {} {y} {z} {} {} {z} {g}", x - 1, x - 1, y + 1),
        format!("fill {} {y} {z} {} {} {z} {g}", x + 1, x + 1, y + 1),
        format!("fill {} {} {} {} {} {} {g}", x - 1, y + 2, z - 1, x + 1, y + 2, z + 1),
    ]
}

/// Survival: seeds until taming (event 7), then deadly cookie (event 3).
///
/// The parrot wanders (entity random, not seeded as in the game) and sometimes left the
/// interaction range during the attempts: it is locked in a glass cell two
/// blocks from the player (the interaction with an entity only looks at distance).
#[test]
fn parrot_tamed_then_cookie() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f02parrot", 0, "normal", &[("parrot_spawn_egg", 1), ("wheat_seeds", 64), ("cookie", 1)]);
    allow_commands(&dir);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let [x, y, z] = [p.pos[0].floor() as i32 + 2, p.pos[1].floor() as i32, p.pos[2].floor() as i32];
    wait_chunk(&s, x, z);
    for c in glass_cell(x, y, z) {
        s.link.send_command(&c).unwrap();
    }
    s.link.send_command(&format!("summon minecraft:parrot {} {y} {}", f64::from(x) + 0.5, f64::from(z) + 0.5)).unwrap();
    let parrot = kind_id(&data, "minecraft:parrot");
    let mut id = None;
    let mut refused_cmds = Vec::new();
    drain_until(&s, 10, |e| {
        if let Event::Chat { component, .. } = e
            && component.style.color == Some(0xFF5555)
        {
            refused_cmds.push(format!("{component:?}"));
        }
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == parrot
        {
            id = Some(a.id);
        }
        id.is_some()
    });
    assert!(refused_cmds.is_empty(), "cell refused: {refused_cmds:?}");
    let id = id.expect("parrot did not appear");
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut tamed = false;
    let (mut tries, mut refused) = (0, 0);
    let mut other: Vec<String> = Vec::new();
    while !tamed && tries < 60 {
        tries += 1;
        s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
        tamed = drain_until(&s, 1, |e| match e {
            Event::Entity(EntityEvent::Event { id: i, event: 6 }) if *i == id => {
                refused += 1;
                false
            }
            Event::Entity(EntityEvent::Event { id: i, event: 7 }) => *i == id,
            Event::Entity(EntityEvent::Remove(l)) if l.contains(&id) => {
                other.push(format!("removed (try {tries})"));
                false
            }
            Event::Entity(EntityEvent::Event { id: i, event }) if *i == id => {
                other.push(format!("event {event} (try {tries})"));
                false
            }
            Event::Inventory(inv) => {
                other.push(format!("inventory (try {tries}) {:.80}", format!("{inv:?}")));
                false
            }
            _ => false,
        });
    }
    eprintln!("parrot tamed: {tamed} after {tries} seeds ({refused} refusals); {other:?}");
    assert!(tamed, "parrot not tamed");
    s.link.send_carried_item(2).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
    let dead = drain_until(&s, 4, |e| matches!(e, Event::Entity(EntityEvent::Event { id: i, event: 3 }) if *i == id));
    assert!(dead, "the cookie did not kill the parrot");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: bowl → mushroom stew; shears → a cow appears, mushrooms on the ground.
#[test]
fn mooshroom_stew_and_shear() {
    use rc_proto_v777::item::Inventory;
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f02moosh", 0, "normal", &[("mooshroom_spawn_egg", 1), ("bowl", 1), ("shears", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    // Chunks loaded before aiming at the ground.
    std::thread::sleep(Duration::from_millis(1500));
    let id = spawn_egg(&s, &data, p.pos, 2, 0, "minecraft:mooshroom", 2);
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
    let stew = item_id(&data, "minecraft:mushroom_stew");
    let got = drain_until(
        &s,
        3,
        |e| matches!(e, Event::Inventory(Inventory::Slot { stack: Some(st), .. }) if st.item == stew),
    );
    assert!(got, "no stew");
    s.link.send_carried_item(2).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
    let (cow, item) = (kind_id(&data, "minecraft:cow"), kind_id(&data, "minecraft:item"));
    let (mut new_cow, mut items, mut gone) = (false, 0, false);
    let (mut cow_id, mut cow_variant) = (None, None);
    drain_until(&s, 4, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == cow => {
                new_cow = true;
                cow_id = Some(a.id);
            }
            Event::Entity(EntityEvent::Data { id, values }) if Some(*id) == cow_id => {
                for (i, v) in values {
                    if let (18, Meta::Variant(n, _)) = (*i, v) {
                        cow_variant = Some(*n);
                    }
                }
            }
            Event::Entity(EntityEvent::Add(a)) if a.kind == item => items += 1,
            Event::Entity(EntityEvent::Remove(ids)) if ids.contains(&id) => gone = true,
            _ => {}
        }
        new_cow && gone && items >= 5
    });
    eprintln!("shearing: cow {new_cow}, mooshroom removed {gone}, items {items}");
    assert!(new_cow && gone && items >= 5, "incomplete shearing");
    // The cow resulting from shearing is "temperate", the default value: never announced (like
    // the game); another variant would be.
    assert_eq!(cow_variant, None, "cow variant announced (expected: temperate by default)");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: the hit bear retaliates; it rears up (data 18) before striking the player.
#[test]
fn polar_bear_stands_and_hits() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f02polar", 0, "normal", &[("polar_bear_spawn_egg", 1), ("wooden_sword", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    // Chunks loaded before aiming at the ground.
    std::thread::sleep(Duration::from_millis(1500));
    let id = spawn_egg(&s, &data, p.pos, 3, 0, "minecraft:polar_bear", 2);
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    s.link.send_attack(id).unwrap();
    let (mut stood, mut hit) = (false, false);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(15) && !(stood && hit) {
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        match e {
            Event::Entity(EntityEvent::Data { id: i, values }) if i == id => {
                if values.iter().any(|(k, v)| *k == 18 && *v == Meta::Bool(true)) {
                    stood = true;
                }
            }
            Event::Health { health, .. } if health < 20.0 => hit = true,
            _ => {}
        }
    }
    eprintln!("bear: reared {stood}, player hit {hit}");
    assert!(stood && hit, "the bear does not retaliate");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: a tamed parrot placed on the player perches on it (player data 19),
/// saved in the player file (`ShoulderEntityLeft`).
///
/// The parrot can only perch after 100 ticks, and its wandering (entity random, not
/// seeded, as in the game) sometimes takes it away from the player during this delay: the player and it
/// therefore share a glass cell of every other block, where their boxes necessarily overlap,
/// whatever the draw.
#[test]
fn parrot_lands_on_shoulder() {
    use super::super::tests::move_player;
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f02shoulder", 0, "peaceful", &[]);
    allow_commands(&dir);
    let (x, z) = (230.5, 230.5);
    move_player(&dir, [x, -59.0, z]);
    let owner = rc_client::offline_uuid("AgentSolo");
    let Nbt::IntArray(o) = crate::nbtio::uuid_nbt(owner) else { unreachable!() };
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    // The player announces they are on the ground (condition to carry a parrot).
    s.link.send_move(rc_client::link::Move::Pos { pos: p.pos }, true, false).unwrap();
    // Cell (230, 230) on y −59..−58: four walls and a roof, then the parrot inside
    // (commands processed in order: the cell exists before its spawn).
    wait_chunk(&s, 230, 230);
    let mut cmds = glass_cell(230, -59, 230);
    cmds.push(format!("summon minecraft:parrot {x} -59 {z} {{Owner:[I;{},{},{},{}]}}", o[0], o[1], o[2], o[3]));
    for c in cmds {
        s.link.send_command(&c).unwrap();
    }
    let mut perched = false;
    let mut refused_cmds = Vec::new();
    drain_until(&s, 15, |e| {
        if let Event::Chat { component, .. } = e
            && component.style.color == Some(0xFF5555)
        {
            refused_cmds.push(format!("{component:?}"));
        }
        if let Event::Entity(EntityEvent::Data { values, .. }) = e
            && values.iter().any(|(i, v)| *i == 19 && matches!(v, Meta::OptionalInt(Some(_))))
        {
            perched = true;
        }
        perched
    });
    eprintln!("parrot on the shoulder: {perched}");
    assert!(refused_cmds.is_empty(), "cell refused: {refused_cmds:?}");
    assert!(perched, "the parrot does not perch");
    s.link.disconnect();
    handle.wait();
    let _ = p;
    let pf = dir.join("players").join("data").join(format!("{}.dat", crate::nbtio::uuid_text(owner)));
    let pl = crate::nbtio::read_dat(&pf).unwrap();
    let left = pl.get("ShoulderEntityLeft").expect("shoulder saved");
    assert_eq!(left.get("id").and_then(Nbt::as_str), Some("minecraft:parrot"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Demo world (`RUSTCRAFT_F02_PREP=<folder>`): the seven species in front of the player
/// (facing south), creative, for a client screenshot in singleplayer.
#[test]
#[ignore]
fn prep_f02_world() {
    let Ok(dir) = std::env::var("RUSTCRAFT_F02_PREP") else { return };
    let Some(world) = test_world() else { return };
    let dir = std::path::PathBuf::from(dir);
    let made = prep(&world, "f02prep", 1, "normal", &[("wheat_seeds", 64), ("bowl", 1), ("shears", 1)]);
    // Player at the center of the zone, facing south; mobs in an arc at 4..7 blocks.
    let (x, z) = (230.5, 230.5);
    for e in std::fs::read_dir(made.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).unwrap();
        crate::nbtio::set(&mut pl, "Pos", Nbt::List([x, -59.0, z].iter().map(|&v| Nbt::Double(v)).collect()));
        crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(15.0)]));
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
    }
    let ai = |extra: Vec<(String, Nbt)>| {
        let mut v = vec![
            ("NoAI".to_owned(), Nbt::Byte(i8::from(std::env::var_os("RUSTCRAFT_F02_AI").is_none()))),
            ("Rotation".to_owned(), Nbt::List(vec![Nbt::Float(180.0), Nbt::Float(0.0)])),
        ];
        v.extend(extra);
        v
    };
    use super::super::tests::{inject, mob_nbt};
    inject(
        &made,
        &[
            mob_nbt("rabbit", [x - 3.0, -59.0, z + 5.0], ai(vec![("RabbitType".into(), Nbt::Int(1))])),
            mob_nbt("mooshroom", [x - 1.5, -59.0, z + 6.5], ai(vec![("Type".into(), Nbt::String("brown".into()))])),
            mob_nbt("bat", [x, -57.0, z + 5.0], ai(vec![("BatFlags".into(), Nbt::Byte(0))])),
            mob_nbt("ocelot", [x + 1.5, -59.0, z + 5.0], ai(Vec::new())),
            mob_nbt("polar_bear", [x + 3.5, -59.0, z + 7.0], ai(Vec::new())),
            mob_nbt("turtle", [x - 3.5, -59.0, z + 7.5], ai(Vec::new())),
            mob_nbt("parrot", [x + 0.5, -59.0, z + 3.5], ai(vec![("Variant".into(), Nbt::Int(2))])),
        ],
    );
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&made, &dir).unwrap();
    eprintln!("world prepared: {}", dir.display());
}

/// Creative: turtle, ocelot, parrot saved with their own fields.
#[test]
fn turtle_ocelot_parrot_saved() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(
        &world,
        "f02save",
        1,
        "normal",
        &[("turtle_spawn_egg", 1), ("ocelot_spawn_egg", 1), ("parrot_spawn_egg", 1)],
    );
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    // Chunks loaded before aiming at the ground.
    std::thread::sleep(Duration::from_millis(1500));
    let _ = spawn_egg(&s, &data, p.pos, 3, 0, "minecraft:turtle", 2);
    let _ = spawn_egg(&s, &data, p.pos, -3, 1, "minecraft:ocelot", 3);
    let _ = spawn_egg(&s, &data, p.pos, 4, 2, "minecraft:parrot", 4);
    std::thread::sleep(Duration::from_secs(3));
    s.link.disconnect();
    handle.wait();
    let (cx, cz) = ((p.pos[0].floor() as i32) >> 4, (p.pos[2].floor() as i32) >> 4);
    let all: Vec<Nbt> = (cx - 1..=cx + 1)
        .flat_map(|x| (cz - 1..=cz + 1).map(move |z| (x, z)))
        .flat_map(|(x, z)| saved(&dir, x, z))
        .collect();
    let find = |n: &str| all.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some(n)).cloned();
    let t = find("minecraft:turtle").expect("turtle saved");
    assert!(matches!(t.get("home_pos"), Some(Nbt::IntArray(a)) if a.len() == 3));
    let o = find("minecraft:ocelot").expect("ocelot saved");
    assert!(o.get("Trusting").is_some());
    let pa = find("minecraft:parrot").expect("parrot saved");
    assert!(pa.get("Variant").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Suffocation (the suffocation test then the "inside a wall" test): a parrot against a pane or in
/// leaves does not suffocate; in stone, soul sand (not full) or a retracted piston, it does.
/// Any collision at eye height used to suffocate mobs, panes included (a parrot that appeared in a
/// pane died in it within a few seconds; in the game, it frees itself).
#[test]
fn suffocating_blocks_follow_the_game() {
    use crate::blocks::probe::Probe;
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = prep(&world, "f02suffoc", 0, "normal", &[]);
    let p = Probe::open(&dir, std::sync::Arc::new(data));
    let s = &p.s;
    let st = |n: &str| s.default_state(n);
    for (name, want) in [
        ("minecraft:stone", true),
        ("minecraft:glass", false),
        ("minecraft:red_stained_glass", false),
        ("minecraft:tinted_glass", false),
        ("minecraft:oak_leaves", false),
        ("minecraft:copper_grate", false),
        ("minecraft:soul_sand", true),
        ("minecraft:mud", true),
        ("minecraft:oak_slab", false),
        ("minecraft:ice", true),
    ] {
        assert_eq!(s.suffocating(st(name)), want, "{name}");
    }
    let piston = st("minecraft:piston");
    assert!(s.suffocating(piston));
    assert!(!s.suffocating(s.v().set_bool(piston, "extended", true)));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}
