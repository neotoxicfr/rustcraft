//! Family F10 end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, flat map): goat
//! (memories and horns saved), tadpole that becomes a frog, frog that swallows a small slime,
//! armadillo that rolls up on hit, axolotl in a bucket.

use std::time::Duration;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::super::tests::{drain_until, kind_id, prep, saved};
use crate::tests::{start_test, test_world};

fn brainy_prep(world: &std::path::Path, tag: &str, mode: i32, items: &[(&str, i32)]) -> std::path::PathBuf {
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

fn added(s: &rc_client::Session, data: &rc_data::VersionData, kind: &str, secs: u64) -> Option<i32> {
    let k = kind_id(data, kind);
    let mut id = None;
    drain_until(s, secs, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == k
        {
            id = Some(a.id);
            return true;
        }
        false
    });
    id
}

/// 8 × 4 × 7 pool east of the player.
fn pool(s: &rc_client::Session, p: [f64; 3]) -> [f64; 3] {
    let (x, y, z) = (p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32);
    cmd(s, &format!("fill {} {} {} {} {} {} minecraft:water", x + 2, y - 4, z - 3, x + 9, y - 1, z + 3));
    [f64::from(x) + 5.5, f64::from(y) - 2.5, f64::from(z) + 0.5]
}

/// Summoned goat: finalization (jump and ram memories, horns), save.
#[test]
fn goat_finalized_and_saved() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = brainy_prep(&world, "f10goat", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, &format!("summon minecraft:goat {} {} {}", p.pos[0] + 3.0, p.pos[1], p.pos[2]));
    let goat = added(&s, &data, "minecraft:goat", 10);
    assert!(goat.is_some(), "goat did not appear");
    std::thread::sleep(Duration::from_millis(1500));
    s.link.disconnect();
    handle.wait();
    let (cx, cz) = ((p.pos[0] + 3.0).floor() as i32 >> 4, p.pos[2].floor() as i32 >> 4);
    let ents = saved(&dir, cx, cz);
    let g = ents.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:goat")).expect("goat saved");
    for k in ["IsScreamingGoat", "HasLeftHorn", "HasRightHorn", "AgeLocked"] {
        assert!(g.get(k).is_some(), "field {k} missing");
    }
    let mem = g.get("Brain").and_then(|b| b.get("memories")).expect("memories");
    let lj = mem.get("minecraft:long_jump_cooling_down").and_then(|v| v.get("value")).and_then(Nbt::as_i64);
    let ram = mem.get("minecraft:ram_cooldown_ticks").and_then(|v| v.get("value")).and_then(Nbt::as_i64);
    eprintln!("goat: jump {lj:?}, ram {ram:?}");
    assert!(lj.is_some_and(|v| (0..=1200).contains(&v)), "jump cooldown");
    assert!(ram.is_some_and(|v| (0..=6000).contains(&v)), "ram cooldown");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Almost adult tadpole: becomes a frog (biome variant, persistent).
#[test]
fn tadpole_grows_into_frog() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = brainy_prep(&world, "f10tadpole", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    cmd(&s, &format!("summon minecraft:tadpole {} {} {} {{Age:23970}}", c[0], c[1], c[2]));
    let tadpole = added(&s, &data, "minecraft:tadpole", 10).expect("tadpole");
    let frog_k = kind_id(&data, "minecraft:frog");
    let (mut frog, mut gone) = (None, false);
    drain_until(&s, 10, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == frog_k => frog = Some(a.id),
            Event::Entity(EntityEvent::Remove(l)) if l.contains(&tadpole) => gone = true,
            _ => {}
        }
        frog.is_some() && gone
    });
    eprintln!("tadpole → frog: {frog:?}, tadpole removed {gone}");
    assert!(frog.is_some() && gone, "no conversion");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Frog and small slime: tongue (pose 9), slime swallowed (removed).
#[test]
fn frog_eats_small_slime() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = brainy_prep(&world, "f10frog", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, &format!("summon minecraft:frog {} {} {}", p.pos[0] + 3.0, p.pos[1], p.pos[2]));
    let frog = added(&s, &data, "minecraft:frog", 10).expect("frog");
    cmd(&s, &format!("summon minecraft:slime {} {} {} {{Size:0,NoAI:1b}}", p.pos[0] + 5.0, p.pos[1], p.pos[2]));
    let slime = added(&s, &data, "minecraft:slime", 10).expect("slime");
    let (mut tongue, mut eaten) = (false, false);
    drain_until(&s, 40, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id, values }) if *id == frog => {
                tongue |= values.iter().any(|(i, v)| *i == 6 && *v == Meta::Pose(9));
            }
            Event::Entity(EntityEvent::Remove(l)) if l.contains(&slime) => eaten = true,
            _ => {}
        }
        tongue && eaten
    });
    eprintln!("frog: tongue {tongue}, slime swallowed {eaten}");
    assert!(tongue && eaten, "the frog does not swallow the slime");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Armadillo hit by the player: rolling up (state 1) then scared (state 2).
#[test]
fn armadillo_rolls_up_when_hit() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = brainy_prep(&world, "f10armadillo", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, &format!("summon minecraft:armadillo {} {} {}", p.pos[0] + 1.5, p.pos[1], p.pos[2]));
    let a = added(&s, &data, "minecraft:armadillo", 10).expect("armadillo");
    std::thread::sleep(Duration::from_millis(1000));
    s.link.send_attack(a).unwrap();
    let (mut rolling, mut scared) = (false, false);
    drain_until(&s, 6, |e| {
        if let Event::Entity(EntityEvent::Data { id, values }) = e
            && *id == a
        {
            for (i, v) in values {
                match (*i, v) {
                    (18, Meta::Variant(1, 36)) => rolling = true,
                    (18, Meta::Variant(2, 36)) => scared = true,
                    _ => {}
                }
            }
        }
        rolling && scared
    });
    eprintln!("armadillo: rolled up {rolling}, scared {scared}");
    assert!(rolling && scared, "the armadillo does not roll up");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Axolotl in a water bucket: removed, axolotl bucket in hand (variant in component).
#[test]
fn axolotl_bucket() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = brainy_prep(&world, "f10axolotl", 0, &[("water_bucket", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let c = pool(&s, p.pos);
    cmd(&s, &format!("summon minecraft:axolotl {} {} {} {{NoAI:1b,Variant:2}}", c[0] - 3.0, c[1] + 1.5, c[2]));
    let a = added(&s, &data, "minecraft:axolotl", 10).expect("axolotl");
    s.link.send_carried_item(0).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut gone = false;
    for _ in 0..20 {
        s.link.send_interact(a, 0, [0.0, 0.1, 0.0], false).unwrap();
        gone = drain_until(&s, 1, |e| matches!(e, Event::Entity(EntityEvent::Remove(l)) if l.contains(&a)));
        if gone {
            break;
        }
    }
    eprintln!("axolotl in bucket: {gone}");
    assert!(gone, "axolotl not picked up");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// F10 demo world (screenshots): pool (axolotl, tadpole), goat on the grass, frog, armadillo,
/// sniffer, allay; AI off except `RUSTCRAFT_F10_AI`.
#[test]
#[ignore]
fn prep_f10_world() {
    let Ok(out) = std::env::var("RUSTCRAFT_F10_PREP") else { return };
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = brainy_prep(&world, "f10prep", 1, &[("water_bucket", 1), ("slime_ball", 16), ("amethyst_shard", 4)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:water", x + 9, y - 3, z - 4, x + 13, y - 1, z + 4));
    let ai = if std::env::var_os("RUSTCRAFT_F10_AI").is_some() { 0 } else { 1 };
    let w = |dx: f64, dy: f64, dz: f64| (f64::from(x) + dx, f64::from(y) + dy, f64::from(z) + dz);
    let mobs = [
        ("goat", w(4.5, 0.0, -4.5), ",HasLeftHorn:0b"),
        ("frog", w(4.5, 0.0, -1.5), ",variant:\"minecraft:warm\""),
        ("armadillo", w(4.5, 0.0, 1.0), ",state:\"scared\""),
        ("sniffer", w(6.5, 0.0, 4.5), ""),
        ("allay", w(3.0, 1.2, 3.0), ",equipment:{mainhand:{id:\"minecraft:diamond\",count:1}}"),
        ("axolotl", w(11.0, -1.5, -2.0), ",Variant:4"),
        ("tadpole", w(11.0, -1.5, 2.0), ""),
    ];
    for (k, (mx, my, mz), extra) in mobs {
        cmd(
            &s,
            &format!(
                "summon minecraft:{k} {mx} {my} {mz} {{NoAI:{ai}b,PersistenceRequired:1b,Rotation:[90f,0f]{extra}}}"
            ),
        );
    }
    cmd(&s, &format!("tp @s {} {} {} -90 30", f64::from(x) - 3.5, f64::from(y), f64::from(z) + 0.5));
    std::thread::sleep(Duration::from_millis(1500));
    s.link.disconnect();
    handle.wait();
    let out = std::path::PathBuf::from(out);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::rename(&dir, &out).unwrap();
    eprintln!("world prepared: {}", out.display());
}

/// Diagnostic: messages of a mob summoned for 20 s (`RUSTCRAFT_F10_DIAG` = species).
#[test]
#[ignore]
fn diag_f10_messages() {
    let Ok(kind) = std::env::var("RUSTCRAFT_F10_DIAG") else { return };
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = brainy_prep(&world, "f10diag", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, &format!("summon minecraft:{kind} {} {} {}", p.pos[0] + 5.0, p.pos[1], p.pos[2]));
    let id = added(&s, &data, &format!("minecraft:{kind}"), 10).expect("mob");
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id: i, values }) if *i == id => eprintln!("data {values:?}"),
            Event::Entity(EntityEvent::Remove(l)) if l.contains(&id) => eprintln!("removed"),
            Event::Entity(EntityEvent::Event { id: i, event }) if *i == id => eprintln!("event {event}"),
            Event::Entity(EntityEvent::Teleport(t)) if t.id == id => eprintln!("teleport {:?}", t),
            _ => {}
        }
        false
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
