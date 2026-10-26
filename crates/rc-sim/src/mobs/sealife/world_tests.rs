//! Family F09 end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, flat map):
//! pool dug with the `fill` command, summoned mobs; swimming, flopping out of
//! water, bucket, puffing and sting, ink, dolphin's fish, taming and mounting of the
//! nautilus, save.

use std::time::Duration;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::play::SoundRef;

use super::super::tests::{drain_until, kind_id, prep, saved};
use crate::tests::{start_test, test_world};

/// Copy of the world, commands allowed.
fn sea_prep(world: &std::path::Path, tag: &str, mode: i32, items: &[(&str, i32)]) -> std::path::PathBuf {
    let dir = prep(world, tag, mode, "normal", items);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    dir
}

/// Chunks loaded and player ready before the first command (a command without effect first).
fn settle(s: &rc_client::Session) {
    std::thread::sleep(Duration::from_millis(2500));
    cmd(s, "time query gametime");
}

fn cmd(s: &rc_client::Session, c: &str) {
    s.link.send_command(c).unwrap();
    std::thread::sleep(Duration::from_millis(150));
}

/// 8 × 4 × 7 pool east of the player (x + 2 to x + 9), four blocks of floor.
fn pool(s: &rc_client::Session, p: [f64; 3]) -> [f64; 3] {
    let (x, y, z) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:water", x + 2, y - 4, z - 3, x + 9, y - 1, z + 3));
    [f64::from(x) + 5.5, f64::from(y) - 2.5, f64::from(z) + 0.5]
}

fn sound_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:sound_event").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

/// Id of the announced mob of this type (bounded wait).
fn added(s: &rc_client::Session, data: &rc_data::VersionData, kind: &str) -> i32 {
    let k = kind_id(data, kind);
    let mut id = None;
    drain_until(s, 10, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == k
        {
            id = Some(a.id);
            return true;
        }
        false
    });
    id.unwrap_or_else(|| panic!("{kind} did not appear"))
}

/// Survival: a cod swims in the pool, a salmon flops on land and suffocates; the cod
/// is picked up with a water bucket.
#[test]
fn fish_swim_flop_and_bucket() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09fish", 0, &[("water_bucket", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    cmd(&s, &format!("summon minecraft:cod {} {} {}", c[0], c[1], c[2]));
    let cod = added(&s, &data, "minecraft:cod");
    cmd(&s, &format!("summon minecraft:salmon {} {} {}", p.pos[0] - 3.0, p.pos[1], p.pos[2]));
    let salmon = added(&s, &data, "minecraft:salmon");
    let flop = sound_id(&data, "minecraft:entity.salmon.flop");
    let (mut moves, mut flops, mut min_air, mut hurt) = (0, 0, 300, false);
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Move(m)) if m.id == cod && m.delta.is_some() => moves += 1,
            Event::Sound(sp) if sp.sound == SoundRef::Id(flop) => flops += 1,
            Event::Entity(EntityEvent::Data { id, values }) if *id == salmon => {
                for (i, v) in values {
                    if let (1, Meta::Int(a)) = (*i, v) {
                        min_air = min_air.min(*a);
                    }
                }
            }
            Event::Entity(EntityEvent::Hurt { id, .. }) if *id == salmon => hurt = true,
            _ => {}
        }
        moves >= 5 && flops >= 3 && hurt
    });
    eprintln!("cod: {moves} moves; salmon: {flops} flops, min air {min_air}, hurt {hurt}");
    assert!(moves >= 5, "the cod does not swim");
    assert!(flops >= 3, "the salmon does not flop");
    assert!(min_air < 300 && hurt, "the salmon does not suffocate");
    // Water bucket: the cod disappears.
    s.link.send_carried_item(0).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    cmd(&s, &format!("tp @s {} {} {}", c[0] - 2.0, p.pos[1], c[2]));
    std::thread::sleep(Duration::from_millis(500));
    let mut gone = false;
    for _ in 0..20 {
        s.link.send_interact(cod, 0, [0.0, 0.1, 0.0], false).unwrap();
        gone = drain_until(&s, 1, |e| matches!(e, Event::Entity(EntityEvent::Remove(l)) if l.contains(&cod)));
        if gone {
            break;
        }
    }
    eprintln!("cod in bucket: {gone}");
    assert!(gone, "cod not picked up");
    // Cod bucket emptied on the ground, two blocks to the west: water placed, cod "from the bucket".
    cmd(&s, &format!("tp @s {} {} {}", p.pos[0], p.pos[1], p.pos[2]));
    drain_until(&s, 3, |e| {
        if let Event::Teleport(t) = e {
            s.link.send_accept_teleport(t.id, t.pos, t.yaw, t.pitch).unwrap();
            return true;
        }
        false
    });
    std::thread::sleep(Duration::from_millis(300));
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    s.link.send_use_item_on([x - 2, y - 1, z], 1, [0.5, 1.0, 0.5], false, 7).unwrap();
    // W8c: like the game, the bucket acts on use (`use_item`), aimed at the top of the block.
    let (dx, dy, dz) =
        (f64::from(x - 2) + 0.5 - p.pos[0], f64::from(y) - (p.pos[1] + 1.62), f64::from(z) + 0.5 - p.pos[2]);
    let yaw = (-dx).atan2(dz).to_degrees() as f32;
    let pitch = (-dy).atan2((dx * dx + dz * dz).sqrt()).to_degrees() as f32;
    s.link.send_use_item(0, 8, yaw, pitch).unwrap();
    let k = kind_id(&data, "minecraft:cod");
    let (mut back, mut from_bucket, mut empty_snd) = (None, false, false);
    let empty = sound_id(&data, "minecraft:item.bucket.empty_fish");
    drain_until(&s, 5, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == k => back = Some(a.id),
            Event::Entity(EntityEvent::Data { id, values }) if Some(*id) == back => {
                from_bucket |= values.iter().any(|(i, v)| *i == 16 && matches!(v, Meta::Bool(true)));
            }
            Event::Sound(sp) if sp.sound == SoundRef::Id(empty) => empty_snd = true,
            _ => {}
        }
        back.is_some() && from_bucket && empty_snd
    });
    eprintln!("bucket emptied: cod {back:?}, from bucket {from_bucket}, sound {empty_snd}");
    assert!(back.is_some() && from_bucket && empty_snd, "cod bucket not emptied");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: the pufferfish puffs near the player (states 1 then 2) and stings them (event 9).
#[test]
fn pufferfish_puffs_and_stings() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09puff", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, &format!("summon minecraft:pufferfish {} {} {}", p.pos[0] + 0.8, p.pos[1] + 0.5, p.pos[2]));
    let fish = added(&s, &data, "minecraft:pufferfish");
    let (mut puff1, mut puff2, mut sting) = (false, false, false);
    drain_until(&s, 15, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id, values }) if *id == fish => {
                for (i, v) in values {
                    match (*i, v) {
                        (17, Meta::Int(1)) => puff1 = true,
                        (17, Meta::Int(2)) => puff2 = true,
                        _ => {}
                    }
                }
            }
            Event::GameEvent { kind: 9, .. } => sting = true,
            _ => {}
        }
        puff1 && puff2 && sting
    });
    eprintln!("pufferfish: puffed {puff1}/{puff2}, sting {sting}");
    assert!(puff1 && puff2, "the pufferfish does not puff");
    assert!(sting, "no sting");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: hit on the squid → ink (particles); hit on the glow squid → dark 100 ticks.
#[test]
fn squid_ink_glow_dark() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09squid", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let _ = pool(&s, p.pos);
    let x = p.pos[0].floor() + 2.5;
    cmd(&s, &format!("summon minecraft:squid {} {} {}", x, p.pos[1] - 1.5, p.pos[2] - 1.0));
    let squid = added(&s, &data, "minecraft:squid");
    cmd(&s, &format!("summon minecraft:glow_squid {} {} {}", x, p.pos[1] - 1.5, p.pos[2] + 1.0));
    let glow = added(&s, &data, "minecraft:glow_squid");
    std::thread::sleep(Duration::from_millis(300));
    let ink = data.registry("minecraft:particle_type").unwrap().entries.iter().position(|e| e == "minecraft:squid_ink");
    let ink = ink.unwrap() as i32;
    let mut inked = 0;
    let mut dark = false;
    let mut moves = 0;
    s.link.send_attack(squid).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    s.link.send_attack(glow).unwrap();
    drain_until(&s, 6, |e| {
        match e {
            Event::Particles(pp) if pp.particle.kind == ink => inked += 1,
            Event::Entity(EntityEvent::Data { id, values }) if *id == glow => {
                if values.iter().any(|(i, v)| *i == 18 && matches!(v, Meta::Int(n) if *n > 90)) {
                    dark = true;
                }
            }
            Event::Entity(EntityEvent::Move(m)) if m.id == squid && m.delta.is_some() => moves += 1,
            _ => {}
        }
        inked >= 30 && dark && moves >= 3
    });
    eprintln!("squid: {inked} ink particles, {moves} moves; glow squid dark {dark}");
    assert!(inked >= 30, "no ink");
    assert!(dark, "the glow squid does not darken");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: the dolphin has 4,800 air, moisture 2,400; it takes the cod (fish received).
#[test]
fn dolphin_air_and_fish() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09dolphin", 0, &[("cod", 4)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    let x = p.pos[0].floor() + 2.5;
    cmd(&s, &format!("summon minecraft:dolphin {} {} {}", x, c[1], c[2]));
    let d = added(&s, &data, "minecraft:dolphin");
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_carried_item(0).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let (mut air, mut fish) = (0, false);
    for _ in 0..10 {
        s.link.send_interact(d, 0, [0.0, 0.3, 0.0], false).unwrap();
        drain_until(&s, 1, |e| {
            if let Event::Entity(EntityEvent::Data { id, values }) = e
                && *id == d
            {
                for (i, v) in values {
                    match (*i, v) {
                        (1, Meta::Int(a)) => air = air.max(*a),
                        (18, Meta::Bool(true)) => fish = true,
                        _ => {}
                    }
                }
            }
            fish
        });
        if fish {
            break;
        }
    }
    eprintln!("dolphin: max air seen {air}, fish received {fish}");
    assert!(fish, "the dolphin does not take the cod");
    s.link.disconnect();
    handle.wait();
    let saved_d = saved(&dir, (c[0].floor() as i32) >> 4, (c[2].floor() as i32) >> 4);
    let e = saved_d.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:dolphin"));
    if let Some(e) = e {
        assert_eq!(crate::nbtio::int(e, "GotFish"), Some(1));
        assert!(crate::nbtio::int(e, "Air").is_some_and(|a| a > 300), "dolphin air not kept");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: nautilus tamed with a pufferfish (event 7), saddled, mounted.
#[test]
fn nautilus_tame_saddle_ride() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09naut", 0, &[("pufferfish", 64), ("saddle", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    let x = p.pos[0].floor() + 2.5;
    cmd(&s, &format!("summon minecraft:nautilus {} {} {} {{NoAI:1b}}", x, c[1], c[2]));
    let n = added(&s, &data, "minecraft:nautilus");
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_carried_item(0).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let mut tamed = false;
    let mut tries = 0;
    while !tamed && tries < 40 {
        tries += 1;
        s.link.send_interact(n, 0, [0.0, 0.4, 0.0], false).unwrap();
        tamed = drain_until(&s, 1, |e| matches!(e, Event::Entity(EntityEvent::Event { id, event: 7 }) if *id == n));
    }
    eprintln!("nautilus tamed: {tamed} after {tries} pufferfish");
    assert!(tamed, "nautilus not tamed");
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(n, 0, [0.0, 0.4, 0.0], false).unwrap();
    let saddled = drain_until(
        &s,
        3,
        |e| matches!(e, Event::Entity(EntityEvent::Equipment { id, slots }) if *id == n && slots.iter().any(|(k, _)| *k == 7)),
    );
    eprintln!("nautilus saddled: {saddled}");
    assert!(saddled, "saddle not placed");
    s.link.send_carried_item(2).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(n, 0, [0.0, 0.4, 0.0], false).unwrap();
    let ridden = drain_until(
        &s,
        3,
        |e| matches!(e, Event::Entity(EntityEvent::Passengers { vehicle, passengers }) if *vehicle == n && !passengers.is_empty()),
    );
    eprintln!("nautilus ridden: {ridden}");
    assert!(ridden, "nautilus not ridden");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: own fields saved (salmon size, tropical fish variant, puffing, darkening,
/// moisture, nautilus memories, zombie nautilus variant).
#[test]
fn aquatic_saved() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09save", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    for k in ["salmon", "tropical_fish", "pufferfish", "glow_squid", "dolphin", "nautilus", "zombie_nautilus", "squid"]
    {
        cmd(&s, &format!("summon minecraft:{k} {} {} {}", c[0], c[1], c[2]));
    }
    std::thread::sleep(Duration::from_millis(1500));
    s.link.disconnect();
    handle.wait();
    let all = saved(&dir, (c[0].floor() as i32) >> 4, (c[2].floor() as i32) >> 4);
    let find = |id: &str| all.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some(id)).cloned();
    let salmon = find("minecraft:salmon").expect("salmon saved");
    assert!(matches!(salmon.get("type").and_then(Nbt::as_str), Some("small" | "medium" | "large")));
    assert!(crate::nbtio::int(&salmon, "FromBucket").is_some());
    let tf = find("minecraft:tropical_fish").expect("tropical fish saved");
    let v = crate::nbtio::int(&tf, "Variant").unwrap() as i32;
    assert_eq!(super::fish::normalize(v), v);
    assert!(find("minecraft:pufferfish").is_some_and(|e| crate::nbtio::int(&e, "PuffState").is_some()));
    assert!(find("minecraft:glow_squid").is_some_and(|e| crate::nbtio::int(&e, "DarkTicksRemaining").is_some()));
    assert!(find("minecraft:dolphin").is_some_and(|e| crate::nbtio::int(&e, "Moistness").is_some()));
    let n = find("minecraft:nautilus").expect("nautilus saved");
    let mem = n.get("Brain").and_then(|b| b.get("memories")).expect("memories");
    assert!(mem.get("minecraft:attack_target_cooldown").is_some(), "attack cooldown missing");
    let z = find("minecraft:zombie_nautilus").expect("zombie nautilus saved");
    assert!(z.get("variant").and_then(Nbt::as_str).is_some_and(|v| v.starts_with("minecraft:")));
    assert!(crate::nbtio::int(&z, "Age").is_none(), "the zombie nautilus has no age");
    let sq = find("minecraft:squid").expect("squid saved");
    assert!(crate::nbtio::int(&sq, "InLove").is_none());
    eprintln!("save: salmon {:?}, tropical {v:#x}", salmon.get("type"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: the nine species, AI active, stay in the pool (neither removed nor hurt) for 20 s.
#[test]
fn all_stay_in_pool() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09stay", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    let mut ids = Vec::new();
    // Without pufferfish: the nautiluses would legitimately charge (prey of the tag `nautilus_hostiles`).
    let kinds = ["cod", "salmon", "tropical_fish", "squid", "glow_squid", "dolphin", "nautilus", "zombie_nautilus"];
    for (i, k) in kinds.iter().enumerate() {
        let x = c[0] - 3.0 + i as f64;
        cmd(&s, &format!("summon minecraft:{k} {x} {} {} {{PersistenceRequired:1b}}", c[1], c[2]));
        ids.push((*k, added(&s, &data, &format!("minecraft:{k}"))));
    }
    let mut gone: Vec<&str> = Vec::new();
    let mut hurt: Vec<&str> = Vec::new();
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Remove(l)) => {
                gone.extend(ids.iter().filter(|(_, id)| l.contains(id)).map(|(k, _)| *k));
            }
            Event::Entity(EntityEvent::Hurt { id, .. }) => {
                hurt.extend(ids.iter().filter(|(_, i)| i == id).map(|(k, _)| *k));
            }
            _ => {}
        }
        false
    });
    eprintln!("removed {gone:?}, hurt {hurt:?}");
    assert!(gone.is_empty() && hurt.is_empty(), "mobs lost");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mob summoned with a compound (`NoAI:0b`) lives normally.
#[test]
fn summoned_with_nbt_lives() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09nbt", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    cmd(&s, &format!("summon minecraft:cod {} {} {} {{NoAI:0b,PersistenceRequired:1b}}", c[0], c[1], c[2]));
    let k = kind_id(&data, "minecraft:cod");
    let mut seen = Vec::new();
    drain_until(&s, 4, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == k => seen.push(format!("add {}", a.id)),
            Event::Entity(EntityEvent::Remove(l)) => seen.push(format!("removal {l:?}")),
            Event::Entity(EntityEvent::Hurt { id, .. }) => seen.push(format!("hit {id}")),
            Event::Chat { component, .. } => seen.push(format!("chat {component:?}")),
            _ => {}
        }
        false
    });
    eprintln!("summon with compound: {seen:?}");
    assert!(seen.iter().any(|x| x.starts_with("add")), "summoned cod missing");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Diagnostics: registered mobs of the demo world (`RUSTCRAFT_F09_PREP`).
#[test]
#[ignore]
fn dump_f09_world() {
    let Ok(out) = std::env::var("RUSTCRAFT_F09_PREP") else { return };
    let dir = std::path::PathBuf::from(out);
    for cx in -32..32 {
        for cz in -32..32 {
            for e in saved(&dir, cx, cz) {
                eprintln!(
                    "{cx},{cz} {:?} pos {:?} hp {:?} no AI {:?}",
                    e.get("id").and_then(Nbt::as_str),
                    crate::nbtio::f64s::<3>(e.get("Pos")),
                    crate::nbtio::float(&e, "Health"),
                    crate::nbtio::int(&e, "NoAI")
                );
            }
        }
    }
}

/// F09 demo world (`RUSTCRAFT_F09_PREP`): pool, the nine motionless species (no AI
/// except `RUSTCRAFT_F09_AI`), creative player at the edge, looking east and down.
#[test]
#[ignore]
fn prep_f09_world() {
    let Ok(out) = std::env::var("RUSTCRAFT_F09_PREP") else { return };
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = sea_prep(&world, "f09prep", 1, &[("water_bucket", 1), ("pufferfish", 16), ("saddle", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:water", x + 2, y - 5, z - 5, x + 13, y - 1, z + 5));
    let ai = if std::env::var_os("RUSTCRAFT_F09_AI").is_some() { 0 } else { 1 };
    let w = |dx: f64, dy: f64, dz: f64| (f64::from(x) + dx, f64::from(y) + dy, f64::from(z) + dz);
    let mobs = [
        ("cod", w(3.5, -1.6, -2.5), ""),
        ("salmon", w(3.5, -1.6, 0.5), ",type:\"large\""),
        ("pufferfish", w(3.5, -1.6, 3.5), ",PuffState:2"),
        ("tropical_fish", w(5.5, -1.6, -3.0), ",Variant:67764993"),
        ("squid", w(6.5, -2.5, 0.5), ""),
        ("glow_squid", w(6.5, -2.5, 3.8), ""),
        ("dolphin", w(9.5, -2.0, -2.5), ""),
        ("nautilus", w(10.0, -2.5, 1.0), ""),
        ("zombie_nautilus", w(10.0, -2.5, 4.0), ",variant:\"minecraft:warm\""),
    ];
    for (k, (mx, my, mz), extra) in mobs {
        cmd(
            &s,
            &format!(
                "summon minecraft:{k} {mx} {my} {mz} {{NoAI:{ai}b,PersistenceRequired:1b,Rotation:[90f,0f]{extra}}}"
            ),
        );
    }
    if std::env::var_os("RUSTCRAFT_F09_UNDER").is_some() {
        // Underwater view: player at the bottom of the pool, west side, looking east.
        cmd(&s, &format!("tp @s {} {} {} -90 5", f64::from(x) + 2.5, f64::from(y) - 4.0, f64::from(z) + 0.5));
    } else {
        cmd(&s, &format!("tp @s {} {} {} -90 35", f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5));
    }
    std::thread::sleep(Duration::from_millis(1500));
    s.link.disconnect();
    handle.wait();
    let out = std::path::PathBuf::from(out);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::rename(&dir, &out).unwrap();
    eprintln!("world prepared: {}", out.display());
}
