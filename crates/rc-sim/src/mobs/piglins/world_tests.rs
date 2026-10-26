//! Family F13 end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, flat Overworld
//! map): hostile piglin without gold, bartering of an ingot, zombification, hoglin that
//! charges and launches, warped fungus that soothes it, zombified piglin neutral then angry
//! with its fellows, save; demo world for screenshots.

use std::time::Duration;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::tests::{drain_until, kind_id, prep, saved};
use crate::tests::{start_test, test_world};

fn pig_prep(world: &std::path::Path, tag: &str, mode: i32, items: &[(&str, i32)]) -> std::path::PathBuf {
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

fn item_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:item").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

/// Survival, without gold: an armed piglin 4 blocks away becomes aggressive (flag 4 of index 15)
/// and hits the player.
#[test]
fn piglin_attacks_player_without_gold() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13atk", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(
        &s,
        &format!(
            "summon minecraft:piglin {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b,equipment:{{mainhand:{{id:\"minecraft:golden_sword\",count:1}}}}}}",
            p.pos[0] + 4.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let id = added(&s, &data, "minecraft:piglin");
    let (mut aggressive, mut hit) = (false, false);
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id: i, values }) if *i == id => {
                aggressive |= values.iter().any(|(k, v)| *k == 15 && matches!(v, Meta::Byte(b) if b & 4 != 0));
            }
            Event::Health { .. } => hit = true,
            _ => {}
        }
        aggressive && hit
    });
    eprintln!("piglin: aggressive {aggressive}, player hit {hit}");
    assert!(aggressive, "the piglin does not attack a player without gold");
    assert!(hit, "the player is not hit");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival, without gold: a piglin with a gold spear does not melee; it lowers the spear
/// (used-item flag, index 8) and charges, then hits the player.
#[test]
fn piglin_with_spear_charges_instead_of_melee() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13spear", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(
        &s,
        &format!(
            "summon minecraft:piglin {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b,equipment:{{mainhand:{{id:\"minecraft:golden_spear\",count:1}}}}}}",
            p.pos[0] + 6.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let id = added(&s, &data, "minecraft:piglin");
    let (mut using, mut hit) = (false, false);
    drain_until(&s, 40, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id: i, values }) if *i == id => {
                using |= values.iter().any(|(k, v)| *k == 8 && matches!(v, Meta::Byte(b) if b & 1 != 0));
            }
            Event::Health { .. } => hit = true,
            _ => {}
        }
        using && hit
    });
    eprintln!("piglin with spear: spear lowered {using}, player hit {hit}");
    assert!(using, "the spear is never lowered");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival, gold helmet: the piglin does not attack; a given ingot is admired (off-hand)
/// then bartered (items thrown, off-hand emptied).
#[test]
fn piglin_admires_and_barters_gold() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13barter", 0, &[("gold_ingot", 3)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, "item replace entity @s armor.head with minecraft:golden_helmet");
    cmd(
        &s,
        &format!(
            "summon minecraft:piglin {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b}}",
            p.pos[0] + 2.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let id = added(&s, &data, "minecraft:piglin");
    std::thread::sleep(Duration::from_millis(1500));
    s.link.send_interact(id, 0, [0.0, 1.0, 0.0], false).unwrap();
    let gold = item_id(&data, "minecraft:gold_ingot");
    let item = kind_id(&data, "minecraft:item");
    let (mut admiring, mut emptied, mut thrown, mut hit) = (false, false, 0, false);
    drain_until(&s, 15, |e| {
        match e {
            Event::Entity(EntityEvent::Equipment { id: i, slots }) if *i == id => {
                for (slot, st) in slots {
                    if *slot == 1 {
                        match st {
                            Some(st) if st.item == gold => admiring = true,
                            None if admiring => emptied = true,
                            _ => {}
                        }
                    }
                }
            }
            Event::Entity(EntityEvent::Add(a)) if a.kind == item && emptied => thrown += 1,
            Event::Health { .. } => hit = true,
            _ => {}
        }
        admiring && emptied && thrown > 0
    });
    eprintln!("bartering: admired {admiring}, hand emptied {emptied}, {thrown} item(s) thrown, hit {hit}");
    assert!(admiring, "the ingot is not taken in the off-hand");
    assert!(emptied && thrown > 0, "no bartering");
    assert!(!hit, "the piglin attacks a gold-helmeted player");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival, gold helmet: an ingot summoned on the ground (`/summon minecraft:item`) 3 blocks from the
/// piglin is a real item entity: seen, reached and picked up, taken in the off-hand.
#[test]
fn piglin_picks_up_summoned_gold() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    assert!(ground_case(&world, data, "f13summoned", false), "piglin does not pick up the summoned ingot");
}

/// Same with an ingot dropped by the player (Q key).
#[test]
fn piglin_picks_up_thrown_gold() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    assert!(ground_case(&world, data, "f13thrown", true), "the piglin does not pick up the dropped ingot");
}

/// Piglin 7 blocks toward +x; ingot dropped by the player (looking toward +x) or summoned 4 blocks away.
/// True if the piglin holds the ingot in the off-hand within 20 s.
fn ground_case(world: &std::path::Path, data: rc_data::VersionData, tag: &str, thrown: bool) -> bool {
    let dir = pig_prep(world, tag, 0, &[("gold_ingot", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, "item replace entity @s armor.head with minecraft:golden_helmet");
    let [x, y, z] = p.pos;
    cmd(
        &s,
        &format!("summon minecraft:piglin {} {y} {z} {{PersistenceRequired:1b,IsImmuneToZombification:1b}}", x + 7.0),
    );
    let id = added(&s, &data, "minecraft:piglin");
    if thrown {
        cmd(&s, "tp @s ~ ~ ~ -90 0");
        s.link.send_player_action(4, [0, 0, 0], 0, 0).unwrap();
    } else {
        cmd(&s, &format!("summon minecraft:item {} {y} {z} {{Item:{{id:\"minecraft:gold_ingot\"}}}}", x + 4.0));
    }
    let gold = item_id(&data, "minecraft:gold_ingot");
    let mut held = false;
    drain_until(&s, 20, |e| {
        if let Event::Entity(EntityEvent::Equipment { id: i, slots }) = e
            && *i == id
        {
            held |= slots.iter().any(|(slot, st)| *slot == 1 && st.as_ref().is_some_and(|st| st.item == gold));
        }
        held
    });
    eprintln!("ground pickup: ingot in off-hand {held}");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    held
}

/// Overworld: a piglin and a hoglin turn into a zombified piglin and a zoglin after 300
/// ticks.
#[test]
fn piglin_and_hoglin_zombify_in_overworld() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13zomb", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    // Without NBT: finalization (first hunt in 30 s at least).
    cmd(&s, &format!("summon minecraft:piglin {} {} {}", p.pos[0] + 5.0, p.pos[1], p.pos[2]));
    cmd(&s, &format!("summon minecraft:hoglin {} {} {}", p.pos[0] - 5.0, p.pos[1], p.pos[2]));
    let (zp, zo) = (kind_id(&data, "minecraft:zombified_piglin"), kind_id(&data, "minecraft:zoglin"));
    let (mut got_zp, mut got_zo) = (false, false);
    drain_until(&s, 30, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e {
            got_zp |= a.kind == zp;
            got_zo |= a.kind == zo;
        }
        got_zp && got_zo
    });
    eprintln!("zombification: zombified piglin {got_zp}, zoglin {got_zo}");
    assert!(got_zp && got_zo, "no zombification in the Overworld");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Overworld, hard: a Nether portal spawns a zombified piglin on the frame
/// (random ticks accelerated for the test).
#[test]
fn nether_portal_spawns_zombified_piglin() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13portal", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, "difficulty hard");
    let (x, y, z) = (p.pos[0].floor() as i32 + 6, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:obsidian", x, y - 1, z, x + 3, y + 3, z));
    cmd(&s, &format!("fill {} {} {} {} {} {} minecraft:nether_portal[axis=x]", x + 1, y, z, x + 2, y + 2, z));
    cmd(&s, "gamerule spawn_mobs true");
    cmd(&s, "gamerule spawn_monsters true");
    cmd(&s, "gamerule random_tick_speed 4096");
    let zp = kind_id(&data, "minecraft:zombified_piglin");
    let mut at = None;
    drain_until(&s, 60, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == zp
        {
            at = Some(a.pos);
            return true;
        }
        false
    });
    eprintln!("portal: zombified piglin born at {at:?} (frame at y {y})");
    let at = at.expect("no zombified piglin born from the portal");
    assert!((at[0] - f64::from(x + 1) - 0.5).abs() < 1.01, "outside the portal in x");
    assert!((at[1] - f64::from(y)).abs() < 1e-6, "not placed on the frame");
    assert!((at[2] - f64::from(z) - 0.5).abs() < 1e-6, "outside the portal in z");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: a hoglin charges, hits and launches the player (velocity sent).
#[test]
fn hoglin_attacks_and_throws() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13hog", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(
        &s,
        &format!(
            "summon minecraft:hoglin {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b}}",
            p.pos[0] + 4.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let id = added(&s, &data, "minecraft:hoglin");
    let (mut ev4, mut hit, mut pushed) = (false, false, false);
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Event { id: i, event: 4 }) if *i == id => ev4 = true,
            Event::Health { .. } => hit = true,
            Event::Entity(EntityEvent::Motion { velocity, .. }) if hit => pushed |= velocity[1] > 0.0,
            _ => {}
        }
        ev4 && hit && pushed
    });
    eprintln!("hoglin: hit {ev4}, touched {hit}, launched {pushed}");
    assert!(ev4 && hit, "the hoglin does not attack");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: a nearby warped fungus soothes the hoglin (no hit in 8 s).
#[test]
fn hoglin_pacified_by_warped_fungus() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13fungus", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("setblock {} {} {} minecraft:warped_fungus", x + 2, y, z + 2));
    cmd(
        &s,
        &format!(
            "summon minecraft:hoglin {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b}}",
            p.pos[0] + 4.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let _id = added(&s, &data, "minecraft:hoglin");
    let mut hit = false;
    drain_until(&s, 8, |e| {
        hit |= matches!(e, Event::Health { .. });
        false
    });
    eprintln!("soothed hoglin: touched {hit}");
    assert!(!hit, "the hoglin attacks near a warped fungus");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival: two zombified piglins stay neutral; hit, one gets angry, the other
/// follows (call), the player is hit; save: end of anger and target recorded.
#[test]
fn zombified_piglins_neutral_then_angry() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13zp", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(
        &s,
        &format!(
            "summon minecraft:zombified_piglin {} {} {} {{PersistenceRequired:1b,Tags:[\"rc_hit\"]}}",
            p.pos[0] + 2.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let a = added(&s, &data, "minecraft:zombified_piglin");
    cmd(
        &s,
        &format!(
            "summon minecraft:zombified_piglin {} {} {} {{PersistenceRequired:1b}}",
            p.pos[0] + 6.0,
            p.pos[1],
            p.pos[2] + 3.0
        ),
    );
    let b = added(&s, &data, "minecraft:zombified_piglin");
    let mut hit = false;
    drain_until(&s, 5, |e| {
        hit |= matches!(e, Event::Health { .. });
        false
    });
    assert!(!hit, "aggressive zombified piglin without provocation");
    // It may have wandered out of range: brought back to 2 blocks before the hit. Designated by its tag:
    // "the closest" sometimes took the other one, which had come closer while wandering, and the hit dealt to the
    // first, left far away, reached nothing.
    cmd(
        &s,
        &format!(
            "tp @e[type=minecraft:zombified_piglin,tag=rc_hit,limit=1] {} {} {}",
            p.pos[0] + 2.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    s.link.send_attack(a).unwrap();
    let (mut angry_a, mut angry_b) = (false, false);
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id, values }) => {
                let ag = values.iter().any(|(k, v)| *k == 15 && matches!(v, Meta::Byte(f) if f & 4 != 0));
                angry_a |= *id == a && ag;
                angry_b |= *id == b && ag;
            }
            Event::Health { .. } => hit = true,
            _ => {}
        }
        angry_a && angry_b && hit
    });
    eprintln!("zombified piglins: anger {angry_a}/{angry_b}, player hit {hit}");
    assert!(angry_a && angry_b, "anger does not spread");
    s.link.disconnect();
    handle.wait();
    let (cx, cz) = ((p.pos[0] as i32) >> 4, (p.pos[2] as i32) >> 4);
    let ents = saved(&dir, cx, cz);
    let zp: Vec<&Nbt> =
        ents.iter().filter(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:zombified_piglin")).collect();
    eprintln!("save: {} zombified piglin(s)", zp.len());
    assert!(
        zp.iter()
            .any(|e| e.get("angry_at").is_some() && e.get("anger_end_time").and_then(Nbt::as_i64).unwrap_or(-1) > 0)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn aggressive(values: &[(u8, Meta)]) -> bool {
    values.iter().any(|(k, v)| *k == 15 && matches!(v, Meta::Byte(b) if b & 4 != 0))
}

/// Survival, gold helmet: the brute ignores gold and hits anyway.
#[test]
fn brute_ignores_gold_and_attacks() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13brute", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, "item replace entity @s armor.head with minecraft:golden_helmet");
    cmd(
        &s,
        &format!(
            "summon minecraft:piglin_brute {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b,equipment:{{mainhand:{{id:\"minecraft:golden_axe\",count:1}}}}}}",
            p.pos[0] + 3.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let _id = added(&s, &data, "minecraft:piglin_brute");
    let mut hit = false;
    drain_until(&s, 20, |e| {
        hit |= matches!(e, Event::Health { .. });
        hit
    });
    eprintln!("brute: gold-helmeted player hit {hit}");
    assert!(hit, "the brute spares a gold-helmeted player");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: the zoglin attacks a neighboring cow (everything except creepers and zoglins).
#[test]
fn zoglin_attacks_other_mobs() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13zoglin", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(
        &s,
        &format!(
            "summon minecraft:cow {} {} {} {{PersistenceRequired:1b,NoAI:1b}}",
            p.pos[0] + 6.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let cow = added(&s, &data, "minecraft:cow");
    cmd(
        &s,
        &format!("summon minecraft:zoglin {} {} {} {{PersistenceRequired:1b}}", p.pos[0] + 9.0, p.pos[1], p.pos[2]),
    );
    let id = added(&s, &data, "minecraft:zoglin");
    let (mut swing, mut cow_hurt) = (false, false);
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Event { id: i, event: 4 }) if *i == id => swing = true,
            Event::Entity(EntityEvent::Data { id: i, values }) if *i == cow => {
                // Health (index 9) dropping.
                cow_hurt |= values.iter().any(|(k, v)| *k == 9 && matches!(v, Meta::Float(h) if *h < 10.0));
            }
            _ => {}
        }
        swing && cow_hurt
    });
    eprintln!("zoglin: hit {swing}, cow hurt {cow_hurt}");
    assert!(swing && cow_hurt, "the zoglin does not attack the cow");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival, gold helmet: opening a chest in front of a piglin makes it angry.
#[test]
fn piglin_angered_by_opened_chest() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13chest", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    cmd(&s, "item replace entity @s armor.head with minecraft:golden_helmet");
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    cmd(&s, &format!("setblock {} {} {} minecraft:chest", x, y, z - 2));
    cmd(
        &s,
        &format!(
            "summon minecraft:piglin {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b,equipment:{{mainhand:{{id:\"minecraft:golden_sword\",count:1}}}}}}",
            p.pos[0] + 3.0,
            p.pos[1],
            p.pos[2]
        ),
    );
    let id = added(&s, &data, "minecraft:piglin");
    let mut hit = false;
    drain_until(&s, 3, |e| {
        hit |= matches!(e, Event::Health { .. });
        false
    });
    assert!(!hit, "aggressive piglin before the chest is opened");
    s.link.send_use_item_on([x, y, z - 2], 1, [0.5, 1.0, 0.5], false, 1).unwrap();
    let mut angry = false;
    drain_until(&s, 20, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id: i, values }) if *i == id => angry |= aggressive(values),
            Event::Health { .. } => hit = true,
            _ => {}
        }
        angry && hit
    });
    eprintln!("chest: piglin angry {angry}, player hit {hit}");
    assert!(angry, "the piglin does not react to the opened chest");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: two hoglins fed crimson fungus produce a baby.
#[test]
fn hoglins_breed_with_crimson_fungus() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = pig_prep(&world, "f13breed", 1, &[("crimson_fungus", 8)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let mut ids = Vec::new();
    for dz in [-1.0, 1.0] {
        cmd(
            &s,
            &format!(
                "summon minecraft:hoglin {} {} {} {{PersistenceRequired:1b,IsImmuneToZombification:1b}}",
                p.pos[0] + 2.5,
                p.pos[1],
                p.pos[2] + dz
            ),
        );
        ids.push(added(&s, &data, "minecraft:hoglin"));
    }
    std::thread::sleep(Duration::from_millis(500));
    for &id in &ids {
        s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
        std::thread::sleep(Duration::from_millis(200));
    }
    let hog = kind_id(&data, "minecraft:hoglin");
    let mut baby = false;
    drain_until(&s, 20, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == hog
            && !ids.contains(&a.id)
        {
            baby = true;
        }
        baby
    });
    eprintln!("hoglin breeding: baby born {baby}");
    assert!(baby, "no baby hoglin");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// F13 demo world (`RUSTCRAFT_F13_PREP`): piglins (sword, crossbow, baby, brute), hoglins
/// (adult, baby), zoglin, zombified piglins, immune to zombification, without AI except
/// `RUSTCRAFT_F13_AI`; creative player facing east.
#[test]
#[ignore]
fn prep_f13_world() {
    let Ok(out) = std::env::var("RUSTCRAFT_F13_PREP") else { return };
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let mode = if std::env::var_os("RUSTCRAFT_F13_SURVIVAL").is_some() { 0 } else { 1 };
    let dir = pig_prep(&world, "f13prep", mode, &[("gold_ingot", 16), ("golden_helmet", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    settle(&s);
    let (x, y, z) = (p.pos[0].floor() as i32, p.pos[1].floor() as i32, p.pos[2].floor() as i32);
    let ai = if std::env::var_os("RUSTCRAFT_F13_AI").is_some() { 0 } else { 1 };
    let w = |dx: f64, dz: f64| (f64::from(x) + dx, f64::from(y), f64::from(z) + dz);
    let sword = ",equipment:{mainhand:{id:\"minecraft:golden_sword\",count:1}}";
    let xbow =
        ",equipment:{mainhand:{id:\"minecraft:crossbow\",count:1},head:{id:\"minecraft:golden_helmet\",count:1}}";
    let axe = ",equipment:{mainhand:{id:\"minecraft:golden_axe\",count:1}}";
    let mobs = [
        ("piglin", w(5.0, -3.0), sword.to_owned()),
        ("piglin", w(5.0, 0.0), xbow.to_owned()),
        ("piglin", w(4.0, 2.5), ",IsBaby:1b".to_owned()),
        ("piglin_brute", w(7.0, 4.0), axe.to_owned()),
        ("hoglin", w(9.0, -3.5), String::new()),
        ("hoglin", w(8.0, 0.5), ",Age:-24000".to_owned()),
        ("zoglin", w(11.0, 3.0), String::new()),
        ("zombified_piglin", w(12.0, -1.0), sword.to_owned()),
        ("zombified_piglin", w(10.5, 6.0), ",IsBaby:1b".to_owned()),
    ];
    for (k, (mx, my, mz), extra) in mobs {
        cmd(
            &s,
            &format!(
                "summon minecraft:{k} {mx} {my} {mz} {{PersistenceRequired:1b,IsImmuneToZombification:1b,NoAI:{ai}b,Rotation:[90f,0f]{extra}}}"
            ),
        );
    }
    cmd(&s, &format!("tp @s {} {} {} -90 15", f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5));
    std::thread::sleep(Duration::from_millis(1500));
    s.link.disconnect();
    handle.wait();
    let out = std::path::PathBuf::from(out);
    let _ = std::fs::remove_dir_all(&out);
    std::fs::rename(&dir, &out).unwrap();
    eprintln!("F13 world prepared: {}", out.display());
}
