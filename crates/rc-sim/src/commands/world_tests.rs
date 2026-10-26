//! End-to-end commands on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`): block entity
//! data of `setblock` / `fill` loaded like the game.

use std::time::{Duration, Instant};

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::tests::{fresh_copy, start_test, test_world};

fn with_commands(world: &std::path::Path, tag: &str) -> std::path::PathBuf {
    let dir = fresh_copy(world, tag);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    dir
}

/// Events received for `ms` milliseconds.
fn collect(s: &rc_client::Session, ms: u64) -> Vec<Event> {
    let mut out = Vec::new();
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(ms) {
        if let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) {
            out.push(e);
        }
    }
    out
}

/// Block state announced at `p` (simple or section update).
fn block_set_at(e: &Event, p: [i32; 3]) -> Option<u32> {
    match e {
        Event::BlockUpdate { pos, state } if [pos.x, pos.y, pos.z] == p => Some(*state),
        Event::SectionUpdate { section, changes } if *section == [p[0] >> 4, p[1] >> 4, p[2] >> 4] => {
            let local = (((p[1] & 15) << 8) | ((p[2] & 15) << 4) | (p[0] & 15)) as u16;
            changes.iter().find(|c| c.0 == local).map(|c| c.1)
        }
        _ => None,
    }
}

/// Last block entity data announced at `p`.
fn last_be(events: &[Event], p: [i32; 3]) -> Option<Nbt> {
    events.iter().rev().find_map(|e| match e {
        Event::BlockEntityData { pos, nbt, .. } if *pos == p => Some(nbt.clone().unwrap_or(Nbt::Compound(Vec::new()))),
        _ => None,
    })
}

/// `setblock` of a `{auto:1b}` command block: loading the data makes it automatic,
/// it runs on the next tick (classic map trick). `setblock` of a chest with `{}`
/// on a full chest: the given data replaces everything, the contents are emptied. Same thing
/// through `fill`.
#[test]
fn setblock_loads_block_entity_data() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = with_commands(&world, "cmdsetblock");
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    // Incoming chunks received and discarded.
    let _ = collect(&s, 2000);
    let [x, y, z] = [p.pos[0].floor() as i32 + 3, p.pos[1].floor() as i32, p.pos[2].floor() as i32];
    let send = |c: String| s.link.send_command(&c).unwrap();
    // Automatic command block through setblock.
    let target = [x, y + 2, z];
    send(format!(
        "setblock {x} {y} {z} minecraft:command_block{{auto:1b,Command:\"setblock {} {} {} minecraft:stone\"}}",
        target[0], target[1], target[2]
    ));
    let ev = collect(&s, 1500);
    let seen: Vec<String> = ev
        .iter()
        .filter(|e| matches!(e, Event::Chat { .. } | Event::BlockUpdate { .. } | Event::SectionUpdate { .. }))
        .map(|e| format!("{e:?}").chars().take(160).collect())
        .collect();
    assert!(
        ev.iter().any(|e| block_set_at(e, target).is_some_and(|st| st != 0)),
        "the automatic command block did not run ({} events): {seen:#?}",
        ev.len()
    );
    // Same thing through fill.
    let target2 = [x, y + 2, z + 2];
    send(format!(
        "fill {x} {y} {} {x} {y} {} minecraft:command_block{{auto:1b,Command:\"setblock {} {} {} minecraft:stone\"}}",
        z + 2,
        z + 2,
        target2[0],
        target2[1],
        target2[2]
    ));
    let ev = collect(&s, 1500);
    assert!(
        ev.iter().any(|e| block_set_at(e, target2).is_some_and(|st| st != 0)),
        "fill: the automatic command block did not run"
    );
    // Full chest, then `{}`: replaced, no more items.
    let c = [x + 2, y, z];
    send(format!(
        "setblock {} {} {} minecraft:chest{{Items:[{{Slot:0b,id:\"minecraft:stone\",count:5}}]}}",
        c[0], c[1], c[2]
    ));
    let ev = collect(&s, 600);
    let first = last_be(&ev, c).expect("chest data announced");
    assert_eq!(crate::nbtio::list(first.get("Items")).len(), 1, "chest filled");
    send(format!("setblock {} {} {} minecraft:chest{{}}", c[0], c[1], c[2]));
    let ev = collect(&s, 600);
    let second = last_be(&ev, c).expect("chest data announced again");
    assert!(crate::nbtio::list(second.get("Items")).is_empty(), "contents kept: {second:?}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Chat texts received for `ms` milliseconds (rendered in English).
fn chat_lines(s: &rc_client::Session, ms: u64) -> Vec<String> {
    collect(s, ms)
        .iter()
        .filter_map(|e| match e {
            Event::Chat { component, .. } => Some(super::cmds_ents::plain(component)),
            _ => None,
        })
        .collect()
}

/// `/random reset <id>` makes the sequence restart from its seed; `/random reset *` forgets
/// them all and announces their count. The sequences are the world's (a single file at the root,
/// `data/minecraft/random_sequences.dat`, nothing under `dimensions/`).
#[test]
fn random_reset_restarts_sequences() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = with_commands(&world, "cmdrandom");
    let _ = std::fs::remove_file(dir.join("data/minecraft/random_sequences.dat"));
    let _ = std::fs::remove_file(dir.join("dimensions/minecraft/overworld/data/minecraft/random_sequences.dat"));
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let _ = collect(&s, 2000);
    let send = |c: &str| s.link.send_command(c).unwrap();
    let draw = || {
        send("random value 1..1000000000 rctest:a");
        let lines = chat_lines(&s, 400);
        lines.iter().find_map(|l| l.strip_prefix("Randomized value: ").map(str::to_owned)).expect("value drawn")
    };
    let first: Vec<String> = (0..3).map(|_| draw()).collect();
    send("random reset rctest:a");
    let lines = chat_lines(&s, 400);
    assert!(lines.iter().any(|l| l == "Reset random sequence rctest:a"), "{lines:?}");
    let again: Vec<String> = (0..3).map(|_| draw()).collect();
    assert_eq!(first, again, "the sequence did not restart from its seed");
    send("random value 1..10 rctest:b");
    let _ = chat_lines(&s, 400);
    send("random reset *");
    let lines = chat_lines(&s, 400);
    assert!(lines.iter().any(|l| l == "Reset 2 random sequence(s)"), "{lines:?}");
    // After `reset *`, the sequence also restarts from its seed.
    assert_eq!(draw(), first[0]);
    // Changed defaults: salt 7, without the world seed → different values, but identical from one
    // `reset *` to the next.
    send("random reset * 7 false");
    let _ = chat_lines(&s, 400);
    let salted = draw();
    assert_ne!(salted, first[0]);
    send("random reset * 7 false");
    let _ = chat_lines(&s, 400);
    assert_eq!(draw(), salted);
    s.link.disconnect();
    handle.wait();
    assert!(dir.join("data/minecraft/random_sequences.dat").exists(), "sequences not saved at the root");
    assert!(
        !dir.join("dimensions/minecraft/overworld/data/minecraft/random_sequences.dat").exists(),
        "sequences saved under the dimension"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Loot chest placed with an identifier without a namespace: the table is found and drawn
/// in the sequence it declares (`minecraft:chests/simple_dungeon`), not in a sequence with the
/// raw name; nonexistent table: no sequence created (level random).
#[test]
fn loot_chest_uses_declared_sequence() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = with_commands(&world, "cmdlootseq");
    let _ = std::fs::remove_file(dir.join("data/minecraft/random_sequences.dat"));
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let _ = collect(&s, 2000);
    let [x, y, z] = [p.pos[0].floor() as i32 + 3, p.pos[1].floor() as i32, p.pos[2].floor() as i32];
    let send = |c: String| s.link.send_command(&c).unwrap();
    // Chests opened by the player (loot drawn on opening), then closed again.
    send("gamemode survival".to_owned());
    for (i, (dz, table)) in [(0, "chests/simple_dungeon"), (2, "minecraft:rctest_missing")].into_iter().enumerate() {
        send(format!("setblock {x} {y} {} minecraft:chest{{LootTable:\"{table}\"}}", z + dz));
        let _ = collect(&s, 300);
        s.link.send_use_item_on([x, y, z + dz], 1, [0.5, 1.0, 0.5], false, 40 + i as i32).unwrap();
        let ev = collect(&s, 400);
        let window = ev.iter().find_map(|e| match e {
            Event::OpenScreen { window, .. } => Some(*window),
            _ => None,
        });
        assert!(window.is_some(), "chest {table} not opened");
        s.link.send_container_close(window.unwrap_or(0)).unwrap();
        let _ = collect(&s, 200);
    }
    s.link.disconnect();
    handle.wait();
    let root =
        crate::nbtio::read_dat(&dir.join("data/minecraft/random_sequences.dat")).unwrap_or(Nbt::Compound(Vec::new()));
    let names: Vec<String> = match root.get("data").and_then(|d| d.get("sequences")) {
        Some(Nbt::Compound(f)) => f.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(names.iter().any(|n| n == "minecraft:chests/simple_dungeon"), "{names:?}");
    assert!(!names.iter().any(|n| n == "chests/simple_dungeon" || n.contains("rctest_missing")), "{names:?}");
}

/// Translation keys of the chat messages received.
fn chats(ev: &[Event]) -> Vec<String> {
    ev.iter()
        .filter_map(|e| if let Event::Chat { component, .. } = e { Some(format!("{component:?}")) } else { None })
        .collect()
}

/// Last speed announced for entity `id`.
fn last_motion(ev: &[Event], id: i32) -> Option<[f64; 3]> {
    ev.iter().rev().find_map(|e| match e {
        Event::Entity(rc_client::EntityEvent::Motion { id: i, velocity }) if *i == id => Some(*velocity),
        _ => None,
    })
}

/// Full `/damage`: `by` (knockback from the attacker), `at` (knockback from the position), `from`,
/// `Invulnerable` flag (passed by `out_of_world`), dropped items (5 hit points, netherite fire
/// resistance), attacker not found refused, player hit by a mob.
#[test]
fn damage_command_sources() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = with_commands(&world, "cmddamage");
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let _ = collect(&s, 2000);
    let run = |c: &str, ms: u64| {
        s.link.send_command(c).unwrap();
        collect(&s, ms)
    };
    let _ = run("gamemode survival", 300);
    let [x, y, z] = p.pos;
    let zombie = |tag: &str, dz: f64, extra: &str| {
        format!(
            "summon minecraft:husk {} {y} {} {{NoAI:1b,PersistenceRequired:1b,Tags:[\"{tag}\"]{extra}}}",
            x + 4.0,
            z + dz
        )
    };
    let mut ev = run(&zombie("dmg_by", 0.0, ""), 200);
    ev.extend(run(&zombie("dmg_at", -3.0, ""), 200));
    ev.extend(run(&zombie("dmg_inv", 3.0, ",Invulnerable:1b"), 200));
    let kind_id =
        |n: &str| data.registry("minecraft:entity_type").unwrap().entries.iter().position(|e| e == n).unwrap() as i32;
    let zk = kind_id("minecraft:husk");
    let ik = kind_id("minecraft:item");
    let id_at = |kind: i32, dx: f64, dz: f64| {
        ev.iter()
            .find_map(|e| match e {
                Event::Entity(rc_client::EntityEvent::Add(a))
                    if a.kind == kind && (a.pos[0] - (x + dx)).abs() < 0.1 && (a.pos[2] - (z + dz)).abs() < 0.1 =>
                {
                    Some(a.id)
                }
                _ => None,
            })
            .expect("summoned entity")
    };
    let (z_by, z_at) = (id_at(zk, 4.0, 0.0), id_at(zk, 4.0, -3.0));
    // `by @s`: knockback towards the east (the player is to the west).
    let ev = run("damage @e[tag=dmg_by,limit=1] 2 minecraft:mob_attack by @s", 400);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "by: {:?}", chats(&ev));
    let v = last_motion(&ev, z_by).unwrap_or_else(|| {
        panic!(
            "knockback announced (by) {z_by}: {:?}",
            ev.iter().map(|e| format!("{e:?}").chars().take(120).collect::<String>()).collect::<Vec<_>>()
        )
    });
    assert!(v[0] > 0.1, "knockback from the attacker expected towards +x: {v:?}");
    // `at`: position 10 blocks east of the player, therefore east of the mob: knockback towards the west.
    let ev = run("damage @e[tag=dmg_at,limit=1] 2 minecraft:mob_attack at ~10 ~ ~", 400);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "at: {:?}", chats(&ev));
    let v = last_motion(&ev, z_at).expect("knockback announced (at)");
    assert!(v[0] < -0.1, "knockback from the position expected towards −x: {v:?}");
    // Attacker not found: argument error, not "invulnerable".
    let ev = run("damage @e[tag=dmg_by,limit=1] 1 minecraft:mob_attack by @e[tag=nobody,limit=1]", 300);
    let c = chats(&ev);
    assert!(
        c.iter().any(|c| c.contains("argument.entity.notfound.entity"))
            && !c.iter().any(|c| c.contains("invulnerable")),
        "attacker not found: {c:?}"
    );
    // `from` accepted (direct entity ≠ attacker).
    let ev = run("damage @e[tag=dmg_at,limit=1] 50 minecraft:arrow by @e[tag=dmg_by,limit=1] from @s", 300);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "from: {:?}", chats(&ev));
    // Invulnerable flag.
    let ev = run("damage @e[tag=dmg_inv,limit=1] 2", 300);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.invulnerable")), "inv: {:?}", chats(&ev));
    let ev = run("damage @e[tag=dmg_inv,limit=1] 2 minecraft:out_of_world", 300);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "out_of_world: {:?}", chats(&ev));
    // Dropped items: stone destroyed after 5 points, netherite insensitive to lava.
    let mut ev = run(
        &format!(
            "summon minecraft:item {} {y} {z} {{Item:{{id:\"minecraft:stone\",count:1}},PickupDelay:32767}}",
            x + 2.5
        ),
        200,
    );
    ev.extend(run(
        &format!(
            "summon minecraft:item {} {y} {z} {{Item:{{id:\"minecraft:netherite_ingot\",count:1}},PickupDelay:32767}}",
            x - 2.5
        ),
        300,
    ));
    let stone = ev
        .iter()
        .find_map(|e| match e {
            Event::Entity(rc_client::EntityEvent::Add(a)) if a.kind == ik && a.pos[0] > x => Some(a.id),
            _ => None,
        })
        .expect("stone summoned");
    let near = |dx: f64| format!("@e[type=minecraft:item,limit=1,sort=nearest,x={},y={y},z={z}]", x + dx);
    let ev = run(&format!("damage {} 2", near(2.5)), 300);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "item: {:?}", chats(&ev));
    let ev = run(&format!("damage {} 3", near(2.5)), 400);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "item 2: {:?}", chats(&ev));
    assert!(
        ev.iter().any(|e| matches!(e, Event::Entity(rc_client::EntityEvent::Remove(l)) if l.contains(&stone))),
        "stone not destroyed at 0 hit points"
    );
    let ev = run(&format!("damage {} 2 minecraft:lava", near(-2.5)), 300);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.invulnerable")), "netherite: {:?}", chats(&ev));
    let ev = run(&format!("damage {} 2", near(-2.5)), 300);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "netherite 2: {:?}", chats(&ev));
    // Player hit "by" a mob.
    let ev = run("damage @s 1 minecraft:mob_attack by @e[tag=dmg_by,limit=1]", 300);
    assert!(chats(&ev).iter().any(|c| c.contains("commands.damage.success")), "player: {:?}", chats(&ev));
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Summon with speed: TNT (fuse), orb and snowball are simulated with the NBT `Motion`
/// (announced at spawn) instead of staying frozen; the TNT explodes.
#[test]
fn summon_reads_motion_nbt() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = with_commands(&world, "cmdmotion");
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let _ = collect(&s, 2000);
    let [x, y, z] = p.pos;
    let kind_id =
        |n: &str| data.registry("minecraft:entity_type").unwrap().entries.iter().position(|e| e == n).unwrap() as i32;
    let mut ev = Vec::new();
    for c in [
        format!("summon minecraft:tnt {} {} {z} {{fuse:10,Motion:[0.1d,0.3d,0.0d]}}", x + 3.0, y + 1.0),
        format!("summon minecraft:experience_orb {} {} {z} {{Value:3,Motion:[0.0d,0.2d,0.05d]}}", x - 3.0, y + 1.0),
        format!("summon minecraft:snowball {x} {} {} {{Motion:[0.0d,0.2d,0.5d]}}", y + 2.0, z + 3.0),
    ] {
        s.link.send_command(&c).unwrap();
        ev.extend(collect(&s, 150));
    }
    ev.extend(collect(&s, 1200));
    let added = |kind: &str| {
        ev.iter().find_map(|e| match e {
            Event::Entity(rc_client::EntityEvent::Add(a)) if a.kind == kind_id(kind) => Some((a.id, a.velocity)),
            _ => None,
        })
    };
    let (tnt, v) = added("minecraft:tnt").expect("TNT summoned");
    assert!((v[0] - 0.1).abs() < 1e-3 && (v[1] - 0.3).abs() < 1e-3, "TNT speed: {v:?}");
    let (orb, v) = added("minecraft:experience_orb").expect("orb summoned");
    assert!((v[1] - 0.2).abs() < 1e-3, "orb speed: {v:?}");
    let (ball, v) = added("minecraft:snowball").expect("snowball summoned");
    assert!((v[2] - 0.5).abs() < 1e-3, "ball speed: {v:?}");
    // Simulated: positions reframed by the server, TNT exploded (removed) after 10 ticks.
    let moved = |id: i32| {
        ev.iter().any(|e| {
            matches!(e, Event::Entity(rc_client::EntityEvent::Move(m)) if m.id == id && m.delta.is_some())
                || matches!(e, Event::Entity(rc_client::EntityEvent::Sync(m)) if m.id == id)
        })
    };
    assert!(moved(orb) && moved(ball), "orb and ball tracked");
    assert!(
        ev.iter().any(|e| matches!(e, Event::Entity(rc_client::EntityEvent::Remove(l)) if l.contains(&tnt))),
        "TNT not exploded"
    );
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
