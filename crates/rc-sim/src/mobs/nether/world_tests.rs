//! F12 family end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, flat
//! map): summoned mobs; blaze bursts, shot and reflection of the ghast's ball, sword and
//! wither effect of the wither skeleton, strider placed on lava and chilled on grass, saddle,
//! mounting and boost with the stick, save.

use std::time::Duration;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::tests::{drain_until, kind_id, prep, saved};
use crate::tests::{start_test, test_world};

/// Copy of the world, commands allowed.
fn neth_prep(world: &std::path::Path, tag: &str, mode: i32, items: &[(&str, i32)]) -> std::path::PathBuf {
    let dir = prep(world, tag, mode, "normal", items);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    dir
}

fn settle(s: &rc_client::Session) {
    std::thread::sleep(Duration::from_millis(2500));
    cmd(s, "time query gametime");
}

fn cmd(s: &rc_client::Session, c: &str) {
    s.link.send_command(c).unwrap();
    std::thread::sleep(Duration::from_millis(150));
}

/// Id of the mob announced of this type (bounded wait).
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
    id.unwrap_or_else(|| panic!("{kind} not spawned"))
}

fn item_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:item").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

/// Survival: a blaze at 6 blocks charges up (index 16), shoots small fireballs
/// (event 1018) that hit the player.
#[test]
fn blaze_charges_and_shoots() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = neth_prep(&world, "f12blaze", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, &format!("summon minecraft:blaze {} {} {} {{PersistenceRequired:1b}}", p.pos[0] + 6.0, p.pos[1], p.pos[2]));
    let b = added(&s, &data, "minecraft:blaze");
    let small = kind_id(&data, "minecraft:small_fireball");
    // Each small ball leaves with a random spread (blaze's random source, not seeded like in the
    // game): at 6 blocks, about one in three hits. Eight bursts of three leave a chance of the order
    // of one in five thousand of missing everything; the wait counts shots, not seconds
    // (under load the bursts space out), with a wide time bound.
    let (mut charged, mut shots, mut balls, mut hit, mut first) = (false, 0, 0, false, None);
    drain_until(&s, 90, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id, values }) if *id == b => {
                charged |= values.iter().any(|(i, v)| *i == 16 && matches!(v, Meta::Byte(1)));
            }
            Event::LevelEvent { kind: 1018, .. } => shots += 1,
            Event::Entity(EntityEvent::Add(a)) if a.kind == small => balls += 1,
            Event::Health { .. } => {
                hit = true;
                first.get_or_insert(shots);
            }
            _ => {}
        }
        (charged && shots >= 3 && balls >= 3 && hit) || shots >= 24
    });
    eprintln!("blaze: charged {charged}, {shots} shots, {balls} balls, player hit {hit} (at shot {first:?})");
    assert!(charged, "the blaze does not charge");
    assert!(shots >= 3 && balls >= 3, "no burst of three");
    assert!(hit, "the player is not hit");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: a caged ghast at 15 blocks warns (1015), shoots (1016) a big ball; the player
/// reflects it with a hit while looking at the ghast: the ghast dies (1,000 damage).
#[test]
fn ghast_fireball_reflected() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = neth_prep(&world, "f12ghast", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    // The ghast flies at random (entity's random source, not seeded like in the game) and fast: it sometimes went
    // up to more than 4 blocks from the player (no more target, no shot) or moved away during the flight
    // of the reflected ball. It is therefore put in a glass cage, at its size (4 × 4 × 4, aligned on
    // the grid), front face pierced by a 3 × 3 hole on the line of fire: it sees the player, its
    // ball leaves the cage, the reflected ball goes back through the hole; it cannot get out.
    // At 15 blocks, the ball flies about ten ticks before being within hitting range: its
    // collision margin has finished growing (0.3 at the 8th tick). Reflected earlier, the margin, which still grows
    // by 0.05 per tick, can pass over the ball between two ticks, which then crosses
    // the ghast without touching it (game behavior, reproduced: `docs/quirks-to-reproduce.md`).
    let [px, py, pz] = [p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32];
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:glass hollow", px + 12, py, pz - 2, px + 17, py + 5, pz + 3));
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:air", px + 12, py + 2, pz - 1, px + 12, py + 4, pz + 1));
    let g = [f64::from(px + 15), f64::from(py + 1), f64::from(pz + 1)];
    cmd(&s, &format!("summon minecraft:ghast {} {} {} {{PersistenceRequired:1b}}", g[0], g[1], g[2]));
    // Ghast position tracked along the announcements: it floats at random (entity's random source, not
    // seeded like in the game) and the player must aim where it IS at the time of the reflection, not where it
    // was at the shot; aiming at the old position made the ball miss one time in a few.
    let ghast = kind_id(&data, "minecraft:ghast");
    let mut gpos = None;
    drain_until(&s, 10, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == ghast
        {
            gpos = Some((a.id, a.pos));
        }
        gpos.is_some()
    });
    let (gid, mut gpos) = gpos.expect("minecraft:ghast not spawned");
    let _ = g;
    let track = |e: &Event, who: i32, at: &mut [f64; 3]| match e {
        Event::Entity(EntityEvent::Move(m)) if m.id == who => {
            if let Some(d) = &m.delta {
                *at = d.path(*at).end();
            }
        }
        Event::Entity(EntityEvent::Teleport(t)) if t.id == who => *at = t.pos,
        Event::Entity(EntityEvent::Sync(y)) if y.id == who => *at = y.path.end(),
        _ => {}
    };
    let big = kind_id(&data, "minecraft:fireball");
    let (mut warn, mut shot, mut ball) = (false, false, None);
    drain_until(&s, 20, |e| {
        track(e, gid, &mut gpos);
        match e {
            Event::LevelEvent { kind: 1015, .. } => warn = true,
            Event::LevelEvent { kind: 1016, .. } => shot = true,
            Event::Entity(EntityEvent::Add(a)) if a.kind == big => ball = Some((a.id, a.pos, a.velocity)),
            _ => {}
        }
        warn && shot && ball.is_some()
    });
    eprintln!("ghast: warning {warn}, shot {shot}, ball {ball:?}, ghast at {gpos:?}, player at {:?}", p.pos);
    assert!(warn && shot, "the ghast does not shoot");
    let (ball, mut bpos, v) = ball.expect("big ball missing");
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let d = v.map(|c| c / l);
    let eye = p.pos[1] + 1.62;
    // Repeated hits until the ball comes within range (6 blocks, checked by the server;
    // velocity returned toward the ghast), the gaze turned before each hit toward the ghast's center
    // (2 blocks above its feet). Starting at 10 blocks, the ball then has at least 8 ticks.
    let mut back = false;
    'tries: for _ in 0..200 {
        let (dx, dy, dz) = (gpos[0] - p.pos[0], gpos[1] + 2.0 - eye, gpos[2] - p.pos[2]);
        let yaw = (-dx.atan2(dz)).to_degrees() as f32;
        let pitch = (-dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees() as f32;
        s.link.send_move(rc_client::link::Move::Rot { yaw, pitch }, true, false).unwrap();
        s.link.send_attack(ball).unwrap();
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_millis(50) {
            let Ok(e) = s.events.recv_timeout(Duration::from_millis(10)) else { continue };
            track(&e, gid, &mut gpos);
            track(&e, ball, &mut bpos);
            if let Event::Entity(EntityEvent::Motion { id, velocity }) = e
                && id == ball
                && velocity[0] * d[0] + velocity[1] * d[1] + velocity[2] * d[2] < 0.0
            {
                back = true;
                break 'tries;
            }
        }
    }
    eprintln!("ball reflected: {back} at {bpos:?}, ghast at {gpos:?}");
    let mut seen = Vec::new();
    let dead = drain_until(&s, 5, |e| {
        track(e, ball, &mut bpos);
        track(e, gid, &mut gpos);
        match e {
            Event::Entity(EntityEvent::Motion { id, velocity }) if *id == ball => {
                seen.push(format!("velocity {velocity:?}"))
            }
            Event::Entity(EntityEvent::Remove(l)) if l.contains(&ball) => {
                seen.push(format!("ball removed at {bpos:?}, ghast at {gpos:?}"))
            }
            Event::Entity(EntityEvent::Hurt { id, .. }) => seen.push(format!("hit {id}")),
            Event::Health { health, .. } => seen.push(format!("health {health}")),
            _ => {}
        }
        matches!(e, Event::Entity(EntityEvent::Event { id, event: 3 }) if *id == gid)
    });
    eprintln!("ghast killed by its reflected ball: {dead}; {seen:?}");
    assert!(dead, "ball not reflected onto the ghast");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: the wither skeleton spawns with a stone sword and applies the wither effect to the player.
#[test]
fn wither_skeleton_sword_and_wither() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = neth_prep(&world, "f12wsk", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, &format!("summon minecraft:wither_skeleton {} {} {}", p.pos[0] + 3.0, p.pos[1], p.pos[2]));
    let w = added(&s, &data, "minecraft:wither_skeleton");
    let sword = item_id(&data, "minecraft:stone_sword");
    let wither = data.registry("minecraft:mob_effect").unwrap().entries.iter().position(|e| e == "minecraft:wither");
    let (mut armed, mut effect) = (false, false);
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Equipment { id, slots }) if *id == w => {
                armed |= slots.iter().any(|(k, st)| *k == 0 && st.as_ref().is_some_and(|st| st.item == sword));
            }
            Event::Entity(EntityEvent::Effect { effect: ef, duration, .. }) => {
                effect |= Some(*ef as usize) == wither && *duration > 150;
            }
            _ => {}
        }
        armed && effect
    });
    eprintln!("wither skeleton: sword {armed}, wither effect {effect}");
    assert!(armed, "no stone sword");
    assert!(effect, "no wither effect");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: a strider placed on a lava pool stays there (bottom at the half cell), warm;
/// another on grass is chilled (index 19). Save: ghast (power), wither skeleton
/// (base damage 4), strider (locked age).
#[test]
fn strider_on_lava_and_saved() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = neth_prep(&world, "f12strider", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:lava", x + 3, y - 1, z - 3, x + 9, y - 1, z + 3));
    std::thread::sleep(Duration::from_millis(300));
    let lx = f64::from(x) + 6.5;
    let lz = f64::from(z) + 0.5;
    cmd(&s, &format!("summon minecraft:strider {lx} {} {lz} {{PersistenceRequired:1b,NoAI:1b}}", f64::from(y) - 0.5));
    let on_lava = added(&s, &data, "minecraft:strider");
    cmd(
        &s,
        &format!("summon minecraft:strider {} {} {} {{PersistenceRequired:1b}}", p.pos[0] - 3.0, p.pos[1], p.pos[2]),
    );
    let cold = added(&s, &data, "minecraft:strider");
    cmd(
        &s,
        &format!(
            "summon minecraft:ghast {} {} {} {{PersistenceRequired:1b,ExplosionPower:3b,NoAI:1b}}",
            p.pos[0],
            p.pos[1] + 12.0,
            p.pos[2] + 10.0
        ),
    );
    // Without a compound: finalized (sword, base damage 4).
    cmd(&s, &format!("summon minecraft:wither_skeleton {} {} {}", p.pos[0] - 2.0, p.pos[1], p.pos[2] - 3.0));
    let (mut cold_seen, mut warm_bad) = (false, false);
    drain_until(&s, 4, |e| {
        if let Event::Entity(EntityEvent::Data { id, values }) = e {
            for (i, v) in values {
                if *i == 19 && matches!(v, Meta::Bool(true)) {
                    cold_seen |= *id == cold;
                    warm_bad |= *id == on_lava;
                }
            }
        }
        false
    });
    eprintln!("strider chilled on grass {cold_seen}, on lava {warm_bad}");
    assert!(cold_seen, "the strider on grass is not cold");
    assert!(!warm_bad, "the strider on lava is cold");
    s.link.disconnect();
    handle.wait();
    let mut all = Vec::new();
    for cx in (x >> 4) - 1..=(x >> 4) + 1 {
        for cz in (z >> 4) - 1..=(z >> 4) + 1 {
            all.extend(saved(&dir, cx, cz));
        }
    }
    let find =
        |id: &str| all.iter().filter(|e| e.get("id").and_then(Nbt::as_str) == Some(id)).cloned().collect::<Vec<_>>();
    let striders = find("minecraft:strider");
    let lava_one = striders
        .iter()
        .filter_map(|e| crate::nbtio::f64s::<3>(e.get("Pos")))
        .find(|q| (q[0] - lx).abs() < 1.0 && (q[2] - lz).abs() < 1.0)
        .expect("lava strider saved");
    eprintln!("lava strider: y {:.4} (surface {})", lava_one[1], f64::from(y) - 0.5);
    assert!((lava_one[1] - (f64::from(y) - 0.5)).abs() < 1.0e-3, "the strider sinks or floats");
    assert!(striders.iter().all(|e| crate::nbtio::int(e, "AgeLocked").is_some()), "AgeLocked absent");
    let ghast = find("minecraft:ghast");
    assert!(ghast.iter().any(|e| crate::nbtio::int(e, "ExplosionPower") == Some(3)), "ghast power lost");
    let wsk = find("minecraft:wither_skeleton");
    let base = wsk.first().and_then(|e| {
        crate::nbtio::list(e.get("attributes"))
            .iter()
            .find(|a| a.get("id").and_then(Nbt::as_str) == Some("minecraft:attack_damage"))
            .and_then(|a| a.get("base").cloned())
    });
    eprintln!("wither skeleton: base damage {base:?}");
    assert!(matches!(base, Some(Nbt::Double(b)) if b == 4.0), "wither skeleton base damage");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: saddled strider on lava, mounted (passengers), boost with the stick (index 18 > 0,
/// stick worn by one point).
#[test]
fn strider_ride_and_boost() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = neth_prep(&world, "f12ride", 0, &[("saddle", 1), ("warped_fungus_on_a_stick", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:lava", x + 2, y - 1, z - 3, x + 8, y - 1, z + 3));
    std::thread::sleep(Duration::from_millis(300));
    cmd(
        &s,
        &format!(
            "summon minecraft:strider {} {} {} {{PersistenceRequired:1b}}",
            f64::from(x) + 2.5,
            f64::from(y) - 0.5,
            p.pos[2]
        ),
    );
    let st = added(&s, &data, "minecraft:strider");
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_carried_item(0).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(st, 0, [0.0, 0.8, 0.0], false).unwrap();
    let saddled = drain_until(
        &s,
        3,
        |e| matches!(e, Event::Entity(EntityEvent::Equipment { id, slots }) if *id == st && slots.iter().any(|(k, _)| *k == 7)),
    );
    eprintln!("saddled strider: {saddled}");
    assert!(saddled, "saddle not placed");
    s.link.send_carried_item(1).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    s.link.send_interact(st, 0, [0.0, 0.8, 0.0], false).unwrap();
    let ridden = drain_until(
        &s,
        3,
        |e| matches!(e, Event::Entity(EntityEvent::Passengers { vehicle, passengers }) if *vehicle == st && !passengers.is_empty()),
    );
    eprintln!("mounted strider: {ridden}");
    assert!(ridden, "strider not mounted");
    // One tick for the strider to see the stick in hand, then boost.
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_use_item(0, 9, 0.0, 0.0).unwrap();
    let mut boost = 0;
    drain_until(&s, 3, |e| {
        if let Event::Entity(EntityEvent::Data { id, values }) = e
            && *id == st
        {
            for (i, v) in values {
                if let (18, Meta::Int(t)) = (*i, v) {
                    boost = *t;
                }
            }
        }
        boost > 0
    });
    eprintln!("boost: {boost} ticks");
    assert!((140..=980).contains(&boost), "no boost");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// F12 demo world (`RUSTCRAFT_F12_PREP`): lava pool, striders (one saddled),
/// blaze, ghast and wither skeleton without AI, creative player, gaze toward the east.
#[test]
#[ignore]
fn prep_f12_world() {
    let Ok(out) = std::env::var("RUSTCRAFT_F12_PREP") else { return };
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = neth_prep(&world, "f12prep", 1, &[("warped_fungus_on_a_stick", 1), ("saddle", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:lava", x + 3, y - 1, z - 4, x + 9, y - 1, z + 4));
    let ai = if std::env::var_os("RUSTCRAFT_F12_AI").is_some() { 0 } else { 1 };
    let w = |dx: f64, dy: f64, dz: f64| (f64::from(x) + dx, f64::from(y) + dy, f64::from(z) + dz);
    let mobs = [
        ("strider", w(5.5, -0.5, -2.0), ",equipment:{saddle:{id:\"minecraft:saddle\",count:1}}"),
        ("strider", w(7.5, -0.5, 1.5), ",Age:-24000"),
        ("strider", w(3.0, 0.0, 6.0), ""),
        ("blaze", w(12.0, 1.0, -4.0), ""),
        ("wither_skeleton", w(12.0, 0.0, 3.0), ""),
        ("ghast", w(20.0, 4.0, -2.0), ""),
    ];
    for (k, (mx, my, mz), extra) in mobs {
        cmd(
            &s,
            &format!(
                "summon minecraft:{k} {mx} {my} {mz} {{PersistenceRequired:1b,NoAI:{ai}b,Rotation:[90f,0f]{extra}}}"
            ),
        );
    }
    cmd(&s, &format!("tp @s {} {} {} -90 20", f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5));
    std::thread::sleep(Duration::from_millis(1500));
    s.link.disconnect();
    handle.wait();
    let out = std::path::PathBuf::from(out);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::rename(&dir, &out).unwrap();
    eprintln!("F12 world prepared: {}", out.display());
}
