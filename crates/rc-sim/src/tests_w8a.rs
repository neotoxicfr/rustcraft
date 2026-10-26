//! W8a: client packets specific to singleplayer (difficulty, rules, signs, books), equipment attributes
//! and death messages, end to end on a copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`). Absent: tests ignored.

use std::time::{Duration, Instant};

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::item::ItemStack;

use crate::tests::{fresh_copy, start_test, test_world};

fn wait(s: &rc_client::Session, secs: u64, mut f: impl FnMut(&Event) -> bool) -> Option<Event> {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        if f(&e) {
            return Some(e);
        }
    }
    None
}

fn stack(data: &rc_data::VersionData, name: &str, count: i32) -> ItemStack {
    let item = data.registry("minecraft:item").unwrap().entries.iter().position(|e| e == name).unwrap() as i32;
    ItemStack { item, count, added: Vec::new(), removed: Vec::new() }
}

/// Copy of the world: commands allowed, player in the wanted mode, empty inventory, full health.
fn world_copy(world: &std::path::Path, tag: &str, game_type: i32) -> std::path::PathBuf {
    let dir = fresh_copy(world, tag);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
        let mut ds = d.get("difficulty_settings").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        crate::nbtio::set(&mut ds, "difficulty", Nbt::String("normal".into()));
        crate::nbtio::set(&mut ds, "locked", Nbt::Byte(0));
        crate::nbtio::set(d, "difficulty_settings", ds);
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).unwrap();
        crate::nbtio::set(&mut pl, "playerGameType", Nbt::Int(game_type));
        crate::nbtio::set(&mut pl, "Inventory", Nbt::List(Vec::new()));
        crate::nbtio::set(&mut pl, "Health", Nbt::Float(20.0));
        crate::nbtio::remove(&mut pl, "equipment");
        crate::nbtio::remove(&mut pl, "attributes");
        let mut ab = pl.get("abilities").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for k in ["flying", "invulnerable"] {
            crate::nbtio::set(&mut ab, k, Nbt::Byte(0));
        }
        crate::nbtio::set(&mut pl, "abilities", ab);
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
    }
    dir
}

/// Difficulty changed from the options, lock set then refusal; rules read then written
/// (invalid value ignored, announcement to operators).
#[test]
fn difficulty_and_game_rules() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = world_copy(&world, "w8a-rules", 0);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let link = &s.link;
    link.send_change_difficulty(3).unwrap();
    assert!(
        wait(&s, 3, |e| matches!(e, Event::Difficulty { id: 3, locked: false })).is_some(),
        "hard difficulty not applied"
    );
    link.send_lock_difficulty(true).unwrap();
    assert!(wait(&s, 3, |e| matches!(e, Event::Difficulty { id: 3, locked: true })).is_some(), "lock not set");
    link.send_change_difficulty(1).unwrap();
    assert!(
        wait(&s, 1, |e| matches!(e, Event::Difficulty { id: 1, .. })).is_none(),
        "difficulty changed despite the lock"
    );
    link.send_client_command(2).unwrap();
    let got = wait(&s, 3, |e| matches!(e, Event::GameRuleValues(_)));
    let Some(Event::GameRuleValues(v)) = got else { panic!("no rule values") };
    assert_eq!(v.len(), 58, "rules available");
    assert!(v.iter().any(|(k, x)| k == "minecraft:keep_inventory" && x == "false"));
    let ticks = v.iter().find(|(k, _)| k == "minecraft:random_tick_speed").map(|x| x.1.clone()).unwrap();
    link.send_set_game_rules(&[
        ("minecraft:keep_inventory".into(), "true".into()),
        ("minecraft:random_tick_speed".into(), "-4".into()),
        ("minecraft:spawn_phantoms".into(), "yes".into()),
    ])
    .unwrap();
    let msg = wait(
        &s,
        3,
        |e| matches!(e, Event::Chat { component, .. } if format!("{component:?}").contains("commands.gamerule.set")),
    );
    assert!(msg.is_some(), "no rule announcement");
    link.send_client_command(2).unwrap();
    let Some(Event::GameRuleValues(v)) = wait(&s, 3, |e| matches!(e, Event::GameRuleValues(_))) else {
        panic!("no rule values (2)")
    };
    assert!(v.iter().any(|(k, x)| k == "minecraft:keep_inventory" && x == "true"), "rule not written");
    assert!(v.iter().any(|(k, x)| k == "minecraft:random_tick_speed" && *x == ticks), "bound ignored");
    assert!(v.iter().any(|(k, x)| k == "minecraft:spawn_phantoms" && x == "true"), "invalid value written");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Chestplate and sword worn: armour and attack speed sent with the item's modifiers (game
/// identifiers); attack damage never sent (not synchronised).
#[test]
fn equipment_attributes_sent() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = world_copy(&world, "w8a-attr", 1);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let link = &s.link;
    let reg = data.registry("minecraft:attribute").unwrap();
    let id = |n: &str| reg.entries.iter().position(|e| e == n).unwrap() as i32;
    let (armor, speed, dmg) = (id("minecraft:armor"), id("minecraft:attack_speed"), id("minecraft:attack_damage"));
    link.send_carried_item(0).unwrap();
    link.send_set_creative_slot(6, Some(&stack(&data, "minecraft:diamond_chestplate", 1))).unwrap();
    link.send_set_creative_slot(36, Some(&stack(&data, "minecraft:diamond_sword", 1))).unwrap();
    let mut seen_armor = None;
    let mut seen_speed = None;
    let mut seen_dmg = false;
    let _ = wait(&s, 3, |e| {
        if let Event::Attributes { attributes, .. } = e {
            for a in attributes {
                if a.id == armor {
                    seen_armor = Some(a.clone());
                }
                if a.id == speed {
                    seen_speed = Some(a.clone());
                }
                seen_dmg |= a.id == dmg;
            }
        }
        seen_armor.is_some() && seen_speed.is_some()
    });
    let a = seen_armor.expect("armour not sent");
    assert_eq!(a.base, 0.0);
    assert_eq!(a.modifiers.len(), 1, "chestplate armour: {:?}", a.modifiers);
    assert!(a.modifiers.iter().any(|m| m.id == "minecraft:armor.chestplate" && m.amount == 8.0 && m.operation == 0));
    let sp = seen_speed.expect("attack speed not sent");
    assert_eq!(sp.base, 4.0);
    // −2.4 in widened 32-bit float, like the game (−2.4000000953674316).
    assert!(
        sp.modifiers.iter().any(|m| m.id == "minecraft:base_attack_speed" && m.amount == f64::from(-2.4f32)),
        "{:?}",
        sp.modifiers
    );
    assert!(!seen_dmg, "attack damage sent (attribute not synchronised)");
    // Chestplate removed: armour sent back without a modifier.
    link.send_set_creative_slot(6, None).unwrap();
    let back = wait(
        &s,
        3,
        |e| matches!(e, Event::Attributes { attributes, .. } if attributes.iter().any(|a| a.id == armor && a.modifiers.is_empty())),
    );
    assert!(back.is_some(), "armour not reset to zero");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sign placed: editor opened (front face), received text recorded (colour codes removed),
/// sent back to the client; use afterwards: editor reopened. Signed book and quill: written book in
/// the hand, opened on use.
#[test]
fn signs_and_books() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = world_copy(&world, "w8a-sign", 1);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let link = &s.link;
    link.send_carried_item(0).unwrap();
    link.send_set_creative_slot(36, Some(&stack(&data, "minecraft:oak_sign", 1))).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let [x0, y0, z0] = p.pos;
    let ground = [x0.floor() as i32 + 2, y0 as i32 - 1, z0.floor() as i32];
    let spos = [ground[0], ground[1] + 1, ground[2]];
    link.send_use_item_on(ground, 1, [0.5, 1.0, 0.5], false, 40).unwrap();
    let open = wait(&s, 3, |e| matches!(e, Event::OpenSignEditor { .. }));
    let Some(Event::OpenSignEditor { pos, front }) = open else { panic!("editor not opened on placement") };
    assert_eq!((pos, front), (spos, true));
    link.send_raw(link.ids.sign_update, |w| {
        let lines = ["§cHello".to_owned(), "world".to_owned(), String::new(), String::new()];
        rc_proto_v777::special::write_sign_update(w, rc_proto::BlockPos::new(spos[0], spos[1], spos[2]), &lines, true)
    })
    .unwrap();
    let be = wait(&s, 3, |e| matches!(e, Event::BlockEntityData { pos, nbt: Some(_), .. } if *pos == spos));
    let Some(Event::BlockEntityData { nbt: Some(n), .. }) = be else { panic!("sign text not sent back") };
    let msgs = crate::nbtio::list(n.get("front_text").and_then(|t| t.get("messages"))).to_vec();
    assert_eq!(msgs.first().and_then(Nbt::as_str), Some("Hello"), "{n:?}");
    assert_eq!(msgs.get(1).and_then(Nbt::as_str), Some("world"));
    // Second send without an authorised editor: ignored.
    link.send_raw(link.ids.sign_update, |w| {
        let lines = ["pirate".to_owned(), String::new(), String::new(), String::new()];
        rc_proto_v777::special::write_sign_update(w, rc_proto::BlockPos::new(spos[0], spos[1], spos[2]), &lines, true)
    })
    .unwrap();
    assert!(
        wait(&s, 1, |e| matches!(e, Event::BlockEntityData { pos, .. } if *pos == spos)).is_none(),
        "text accepted without an open editor"
    );
    // Book and quill: pages then signature.
    link.send_set_creative_slot(36, Some(&stack(&data, "minecraft:writable_book", 1))).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    link.send_raw(link.ids.edit_book, |w| {
        rc_proto_v777::special::write_edit_book(w, 0, &["page one".to_owned(), "page two".to_owned()], Some("Title"))
    })
    .unwrap();
    let written = stack(&data, "minecraft:written_book", 1).item;
    let got = wait(
        &s,
        3,
        |e| matches!(e, Event::Inventory(rc_proto_v777::item::Inventory::Slot { slot: 36, stack: Some(st), .. }) if st.item == written),
    );
    assert!(got.is_some(), "book not signed");
    link.send_use_item(0, 41, 0.0, 0.0).unwrap();
    assert!(wait(&s, 3, |e| matches!(e, Event::OpenBook(0))).is_some(), "written book not opened");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Death messages: booby-trapped respawn point ("Intentional Game Design" link), hit by a named
/// mob (custom name, hover), player name with hover and click.
#[test]
fn death_messages() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = world_copy(&world, "w8a-death", 0);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let link = &s.link;
    std::thread::sleep(Duration::from_millis(300));
    link.send_command("damage AgentSolo 100 minecraft:bad_respawn_point").unwrap();
    let died = wait(&s, 8, |e| matches!(e, Event::Died { .. }));
    let Some(Event::Died { message: Some(m), .. }) = died else { panic!("no death") };
    let txt = format!("{m:?}");
    assert!(txt.contains("death.attack.badRespawnPoint.message"), "{txt}");
    assert!(txt.contains("death.attack.badRespawnPoint.link"), "{txt}");
    assert!(txt.contains("MCPE-28723"), "{txt}");
    assert!(txt.contains("AgentSolo") && txt.contains("/tell AgentSolo "), "{txt}");
    link.send_client_command(0).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    link.send_player_loaded().unwrap();
    // Respawn invulnerability (60 ticks) over.
    std::thread::sleep(Duration::from_millis(3500));
    link.send_command("summon zombie ~3 ~ ~ {CustomName:\"Gerald\",NoAI:1b,PersistenceRequired:1b}").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    link.send_command("damage AgentSolo 100 minecraft:mob_attack by @e[type=zombie,limit=1,sort=nearest]").unwrap();
    let died = wait(&s, 8, |e| {
        if let Event::Chat { component, .. } = e {
            eprintln!("chat {component:?}");
        }
        matches!(e, Event::Died { .. })
    });
    let Some(Event::Died { message: Some(m), .. }) = died else { panic!("no second death") };
    let txt = format!("{m:?}");
    assert!(txt.contains("death.attack.mob"), "{txt}");
    assert!(txt.contains("Gerald"), "custom name missing: {txt}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Damage tags are stored under their full name: the disabled `fall_damage` rule must
/// block a fall, and an allowed fall kills with a fall message.
#[test]
fn fall_damage_tag_and_rule() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = world_copy(&world, "w8a-fall", 0);
    let data = std::sync::Arc::new(data);
    let (s, handle, _p) = start_test(&dir, data.clone());
    let link = &s.link;
    std::thread::sleep(Duration::from_millis(300));
    link.send_command("gamerule minecraft:fall_damage false").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    link.send_command("damage AgentSolo 100 minecraft:fall").unwrap();
    let died = wait(&s, 2, |e| matches!(e, Event::Died { .. }));
    assert!(died.is_none(), "fall_damage rule ignored");
    link.send_command("gamerule minecraft:fall_damage true").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    link.send_command("damage AgentSolo 100 minecraft:fall").unwrap();
    let died = wait(&s, 8, |e| matches!(e, Event::Died { .. }));
    let Some(Event::Died { message: Some(m), .. }) = died else { panic!("no death") };
    assert!(format!("{m:?}").contains("death.attack.fall"), "{m:?}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
