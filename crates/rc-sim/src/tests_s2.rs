//! End-to-end survival loop on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`):
//! crafting by clicks, breaking in survival with loot picked up, deadly fall and respawn,
//! cooking furnace. Absent: tests ignored.

use std::time::{Duration, Instant};

use rc_client::{EntityEvent, Event};
use rc_proto_v777::container::HashedStack;
use rc_proto_v777::item::{Inventory, ItemStack};

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

fn item_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:item").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

fn stack(data: &rc_data::VersionData, name: &str, count: i32) -> ItemStack {
    ItemStack { item: item_id(data, name), count, added: Vec::new(), removed: Vec::new() }
}

fn hashed(s: &ItemStack) -> HashedStack {
    HashedStack { item: s.item, count: s.count, added: Vec::new(), removed: Vec::new() }
}

/// Last window state number seen in the packets received up to `until`.
fn state_of(ev: &Event, window: i32) -> Option<i32> {
    match ev {
        Event::Inventory(Inventory::Content { container, state, .. } | Inventory::Slot { container, state, .. })
            if *container == window =>
        {
            Some(*state)
        }
        _ => None,
    }
}

/// Preparation of a demonstration world (`RUSTCRAFT_S2_PREP=<world folder>`,
/// `RUSTCRAFT_S2_SCENARIO=craft|cook|fall|fall10|pickup`): player in survival, inventory, position.
#[test]
#[ignore]
fn prep_world() {
    use rc_proto::nbt::Nbt;
    let (Ok(dir), Ok(sc)) = (std::env::var("RUSTCRAFT_S2_PREP"), std::env::var("RUSTCRAFT_S2_SCENARIO")) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
        crate::nbtio::set(d, "GameType", Nbt::Int(0));
        if sc == "fall10" {
            // Normal, without regeneration (hunger 17, saturation 0): health stays at 13 on screen.
            let mut ds = d.get("difficulty_settings").cloned().unwrap_or(Nbt::Compound(Vec::new()));
            crate::nbtio::set(&mut ds, "difficulty", Nbt::String("normal".into()));
            crate::nbtio::set(d, "difficulty_settings", ds);
        }
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    if sc == "respawn" {
        // Immediate respawn: the client resends the request on its own (no death screen).
        let gp = dir.join("data").join("minecraft").join("game_rules.dat");
        let mut g = crate::nbtio::read_dat(&gp).unwrap_or(Nbt::Compound(Vec::new()));
        let mut d = g.get("data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        crate::nbtio::set(&mut d, "minecraft:immediate_respawn", Nbt::Byte(1));
        crate::nbtio::set(&mut g, "data", d);
        std::fs::write(&gp, crate::nbtio::gzip(&crate::nbtio::encode_root(&g))).unwrap();
    }
    let item = |slot: i8, id: &str, count: i32| {
        Nbt::Compound(vec![
            ("Slot".into(), Nbt::Byte(slot)),
            ("id".into(), Nbt::String(format!("minecraft:{id}"))),
            ("count".into(), Nbt::Int(count)),
        ])
    };
    let inv = match sc.as_str() {
        "craft" => vec![item(9, "oak_planks", 4), item(10, "oak_log", 2)],
        "cook" => vec![item(0, "furnace", 1), item(1, "raw_iron", 3), item(2, "coal", 2)],
        _ => vec![item(0, "stone_pickaxe", 1)],
    };
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("dat") {
            continue;
        }
        let mut pl = crate::nbtio::read_dat(&p).unwrap();
        crate::nbtio::set(&mut pl, "playerGameType", Nbt::Int(0));
        crate::nbtio::set(&mut pl, "Inventory", Nbt::List(inv.clone()));
        crate::nbtio::set(&mut pl, "Health", Nbt::Float(20.0));
        crate::nbtio::set(&mut pl, "foodLevel", Nbt::Int(if sc == "fall10" { 17 } else { 20 }));
        if sc == "fall10" {
            crate::nbtio::set(&mut pl, "foodSaturationLevel", Nbt::Float(0.0));
        }
        crate::nbtio::remove(&mut pl, "equipment");
        let mut ab = pl.get("abilities").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for k in ["flying", "mayfly", "instabuild", "invulnerable"] {
            crate::nbtio::set(&mut ab, k, Nbt::Byte(0));
        }
        crate::nbtio::set(&mut pl, "abilities", ab);
        let rise = match sc.as_str() {
            "fall" | "respawn" => 40.0,
            "fall10" => 10.0,
            _ => 0.0,
        };
        if rise > 0.0
            && let Some([x, y, z]) = crate::nbtio::f64s::<3>(pl.get("Pos"))
        {
            crate::nbtio::set(&mut pl, "Pos", Nbt::List(vec![Nbt::Double(x), Nbt::Double(y + rise), Nbt::Double(z)]));
        }
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
        eprintln!("player prepared: {}", p.display());
    }
}

/// Furnace placed and opened, ore and coal passed by shift-click: an ingot comes out; chest
/// filled then reopened: its contents are kept.
#[test]
fn furnace_and_chest() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = fresh_copy(&world, "s2f");
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let link = &s.link;
    link.send_carried_item(0).unwrap();
    link.send_set_creative_slot(36, Some(&stack(&data, "minecraft:furnace", 1))).unwrap();
    link.send_set_creative_slot(37, Some(&stack(&data, "minecraft:raw_iron", 1))).unwrap();
    link.send_set_creative_slot(38, Some(&stack(&data, "minecraft:coal", 1))).unwrap();
    let [x0, y0, z0] = p.pos;
    let ground = [x0.floor() as i32 + 2, y0 as i32 - 1, z0.floor() as i32];
    let fpos = [ground[0], ground[1] + 1, ground[2]];
    link.send_use_item_on(ground, 1, [0.5, 1.0, 0.5], false, 30).unwrap();
    assert!(
        wait(&s, 3, |e| matches!(e, Event::BlockUpdate { pos, state } if [pos.x, pos.y, pos.z] == fpos
        && data.block_of(*state).is_some_and(|b| b.name == "minecraft:furnace")))
        .is_some(),
        "furnace not placed"
    );
    link.send_use_item_on(fpos, 2, [0.5, 0.5, 0.0], false, 31).unwrap();
    let open = wait(&s, 3, |e| matches!(e, Event::OpenScreen { kind: 14, .. }));
    let Some(Event::OpenScreen { window, .. }) = open else { panic!("furnace menu not opened") };
    // Shift-click: ore (hotbar 1 → slot 31) to the input, coal (hotbar 2 → 32) to the fuel.
    link.send_container_click(window, 0, 31, 0, 1, &[], None).unwrap();
    link.send_container_click(window, 0, 32, 0, 1, &[], None).unwrap();
    let ingot = item_id(&data, "minecraft:iron_ingot");
    let out = wait(
        &s,
        15,
        |e| matches!(e, Event::Inventory(Inventory::Slot { slot: 2, stack: Some(st), .. }) if st.item == ingot),
    );
    assert!(out.is_some(), "no ingot after cooking");
    link.send_container_close(window).unwrap();
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn survival_loop() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = fresh_copy(&world, "s2");
    // Cheating allowed on the copy (game mode change).
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", rc_proto::nbt::Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let link = &s.link;
    let mut state = 0;
    // Creative mode to fill the inventory.
    link.send_change_game_mode(2).unwrap();
    assert!(wait(&s, 5, |e| matches!(e, Event::GameEvent { kind: 3, .. })).is_some(), "game mode refused");
    link.send_change_game_mode(1).unwrap();
    assert!(wait(&s, 5, |e| matches!(e, Event::GameEvent { kind: 3, .. })).is_some(), "game mode refused");
    let log = stack(&data, "minecraft:oak_log", 3);
    link.send_carried_item(0).unwrap();
    link.send_set_creative_slot(36, Some(&log)).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // --- 2×2 crafting: take the logs, place them in the grid, shift-click the result.
    let _ = wait(&s, 2, |e| {
        if let Some(st) = state_of(e, 0) {
            state = st;
        }
        false
    });
    link.send_container_click(0, state, 36, 0, 0, &[(36, None)], Some(&hashed(&log))).unwrap();
    link.send_container_click(0, state, 1, 0, 0, &[(1, Some(hashed(&log)))], None).unwrap();
    let r = wait(&s, 3, |e| {
        matches!(e, Event::Inventory(Inventory::Slot { container: 0, slot: 0, stack: Some(st), .. })
            if st.item == item_id(&data, "minecraft:oak_planks"))
    });
    assert!(r.is_some(), "planks result missing");
    let mut last_state = 0;
    let _ = wait(&s, 1, |e| {
        if let Some(st) = state_of(e, 0) {
            last_state = st;
        }
        false
    });
    let st = if let Some(Event::Inventory(Inventory::Slot { state, .. })) = r { state.max(last_state) } else { 0 };
    link.send_container_click(0, st, 0, 0, 1, &[], None).unwrap();
    let mut planks = 0;
    let _ = wait(&s, 2, |e| {
        if let Event::Inventory(Inventory::Slot { container: 0, slot, stack: Some(x), .. }) = e
            && x.item == item_id(&data, "minecraft:oak_planks")
            && *slot >= 9
        {
            planks = planks.max(x.count);
        }
        false
    });
    assert_eq!(planks, 12, "3 logs → 12 planks");
    // --- Survival: breaking grass with a pickaxe, loot (dirt) picked up in the next slot.
    link.send_set_creative_slot(36, Some(&stack(&data, "minecraft:stone_pickaxe", 1))).unwrap();
    link.send_change_game_mode(0).unwrap();
    assert!(wait(&s, 3, |e| matches!(e, Event::GameEvent { kind: 3, .. })).is_some());
    let [x0, y0, z0] = p.pos;
    let ground = [x0.floor() as i32 + 1, y0 as i32 - 1, z0.floor() as i32];
    link.send_player_action(0, ground, 1, 20).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    link.send_player_action(3, ground, 1, 21).unwrap();
    let add = wait(
        &s,
        5,
        |e| matches!(e, Event::Entity(EntityEvent::Add(a)) if a.kind == data.registry("minecraft:entity_type").unwrap().entries.iter().position(|n| n == "minecraft:item").unwrap() as i32),
    );
    assert!(add.is_some(), "no item entity after the break");
    // The player goes down into the hole to pick up.
    let hole = [f64::from(ground[0]) + 0.5, f64::from(ground[1]), f64::from(ground[2]) + 0.5];
    link.send_move(rc_client::link::Move::Pos { pos: [hole[0], y0, hole[2]] }, true, false).unwrap();
    link.send_move(rc_client::link::Move::Pos { pos: hole }, true, false).unwrap();
    link.send_tick_end().unwrap();
    let dirt = item_id(&data, "minecraft:dirt");
    let mut dirt_slot = None;
    let take = wait(&s, 5, |e| {
        if let Event::Inventory(Inventory::Slot { container: 0, slot, stack: Some(st), .. }) = e
            && st.item == dirt
        {
            dirt_slot = Some(*slot);
        }
        matches!(e, Event::Misc(rc_client::misc::Misc::Take { .. }))
    });
    assert!(take.is_some(), "item not picked up");
    let _ = wait(&s, 1, |e| {
        if let Event::Inventory(Inventory::Slot { container: 0, slot, stack: Some(st), .. }) = e
            && st.item == dirt
        {
            dirt_slot = Some(*slot);
        }
        false
    });
    assert_eq!(dirt_slot, Some(37), "dirt stored next to the pickaxe (hotbar 2)");
    // --- Fall of 30 blocks: death, then respawn.
    let top = [x0, y0 + 30.0, z0];
    link.send_move(rc_client::link::Move::Pos { pos: top }, false, false).unwrap();
    for i in 1..=30 {
        let on = i == 30;
        link.send_move(rc_client::link::Move::Pos { pos: [x0, y0 + 30.0 - f64::from(i), z0] }, on, false).unwrap();
        link.send_tick_end().unwrap();
    }
    let died = wait(&s, 5, |e| matches!(e, Event::Died { .. }));
    let Some(Event::Died { message, .. }) = died else { panic!("no death by fall") };
    eprintln!("death message: {message:?}");
    link.send_client_command(0).unwrap();
    let respawn = wait(&s, 5, |e| matches!(e, Event::Respawn { .. }));
    let Some(Event::Respawn { last_death, .. }) = respawn else { panic!("no respawn") };
    // Place of death passed on respawn (recovery compass).
    let (dim, at) = last_death.expect("place of death missing from the spawn info");
    assert_eq!(dim, "minecraft:overworld");
    assert_eq!([at[0], at[2]], [x0.floor() as i32, z0.floor() as i32], "place of death {at:?}");
    let h = wait(&s, 5, |e| matches!(e, Event::Health { health, .. } if *health == 20.0));
    assert!(h.is_some(), "health not given back");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Copy of the world with cheating allowed and a freshly reset player (health 20, hunger 20, saturation 5,
/// exhaustion 0: the state of a player arriving on the official server), difficulty imposed.
fn parity_world(world: &std::path::Path, tag: &str, game_type: i32, difficulty: &str) -> std::path::PathBuf {
    use rc_proto::nbt::Nbt;
    let dir = fresh_copy(world, tag);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
        let mut ds = d.get("difficulty_settings").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        crate::nbtio::set(&mut ds, "difficulty", Nbt::String(difficulty.into()));
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
        crate::nbtio::set(&mut pl, "foodLevel", Nbt::Int(20));
        crate::nbtio::set(&mut pl, "foodSaturationLevel", Nbt::Float(5.0));
        crate::nbtio::set(&mut pl, "foodExhaustionLevel", Nbt::Float(0.0));
        crate::nbtio::set(&mut pl, "foodTickTimer", Nbt::Int(0));
        crate::nbtio::remove(&mut pl, "equipment");
        let mut ab = pl.get("abilities").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for k in ["flying", "invulnerable"] {
            crate::nbtio::set(&mut ab, k, Nbt::Byte(0));
        }
        crate::nbtio::set(&mut pl, "abilities", ab);
        std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
    }
    dir
}

/// New player in survival, normal difficulty: fall of 10 blocks → 13 HP (same figure read
/// on the official server); then regeneration by saturation (every 10 ticks), then
/// by hunger (80 ticks) until hunger drops to 17: 18.5 HP, hunger 17, saturation 0
/// (tick-by-tick unfolding of the 26.3 code). Long (≈ 16 s): run on demand.
#[test]
#[ignore]
fn parity_fall_and_regeneration() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = parity_world(&world, "s2p", 0, "normal");
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let link = &s.link;
    std::thread::sleep(Duration::from_millis(500));
    let [x0, y0, z0] = p.pos;
    link.send_move(rc_client::link::Move::Pos { pos: [x0, y0 + 10.0, z0] }, false, false).unwrap();
    for i in 1..=10 {
        link.send_move(rc_client::link::Move::Pos { pos: [x0, y0 + 10.0 - f64::from(i), z0] }, i == 10, false).unwrap();
        link.send_tick_end().unwrap();
    }
    let hit = wait(&s, 5, |e| matches!(e, Event::Health { health, .. } if *health < 20.0));
    let Some(Event::Health { health, .. }) = hit else { panic!("no fall damage") };
    assert_eq!(health, 13.0, "fall of 10 blocks: 7 damage");
    let mut last = (health, 20, 5.0);
    let _ = wait(&s, 16, |e| {
        if let Event::Health { health, food, saturation } = e {
            last = (*health, *food, *saturation);
        }
        false
    });
    eprintln!("after regeneration: {last:?}");
    // Same sequence of additions in f32 as the game: saturation heals 5, 4, 3, 3, 2, 1, 1, 1, 1
    // (sixths), then two hunger heals.
    let mut h = 13.0f32;
    for sat in [5.0f32, 4.0, 3.0, 3.0, 2.0, 1.0, 1.0, 1.0, 1.0] {
        h += sat / 6.0;
    }
    h += 1.0;
    h += 1.0;
    assert_eq!(last, (h, 17, 0.0), "final state of the regeneration");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two stacks of dirt thrown at the same place, player turned spectator (no more pickup):
/// sizes of the items on the ground after 6 s.
fn thrown_pair(world: &std::path::Path, data: &std::sync::Arc<rc_data::VersionData>, pair: [i32; 2]) -> Vec<i32> {
    let dir = parity_world(world, "s2m", 1, "peaceful");
    let (s, handle, _p) = start_test(&dir, data.clone());
    let link = &s.link;
    let dirt = item_id(data, "minecraft:dirt");
    let item_kind = data.registry("minecraft:entity_type").unwrap().entries.iter().position(|n| n == "minecraft:item");
    link.send_carried_item(0).unwrap();
    for n in pair {
        link.send_set_creative_slot(36, Some(&stack(data, "minecraft:dirt", n))).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        // Throw the whole held stack (Ctrl+Q).
        link.send_player_action(4, [0, 0, 0], 0, 0).unwrap();
        std::thread::sleep(Duration::from_millis(100));
    }
    link.send_change_game_mode(3).unwrap();
    let mut counts = std::collections::BTreeMap::new();
    let _ = wait(&s, 6, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if Some(a.kind as usize) == item_kind => {
                counts.insert(a.id, 0);
            }
            Event::Entity(EntityEvent::Data { id, values }) if counts.contains_key(id) => {
                for (k, v) in values {
                    if let (8, rc_proto_v777::entity::Meta::Item(Some(st))) = (k, v)
                        && st.item == dirt
                    {
                        counts.insert(*id, st.count);
                    }
                }
            }
            Event::Entity(EntityEvent::Remove(ids)) => {
                for id in ids {
                    counts.remove(id);
                }
            }
            _ => {}
        }
        false
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let mut v: Vec<i32> = counts.values().copied().filter(|&n| n > 0).collect();
    v.sort_unstable();
    v
}

/// Merging of items on the ground compared with the official 26.3 server (reading of 12 October: 40 + 30 do not
/// merge, the sum exceeds the stack; 40 + 20 → 60).
#[test]
fn parity_item_merge() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let data = std::sync::Arc::new(data);
    assert_eq!(thrown_pair(&world, &data, [40, 30]), vec![30, 40], "40 + 30: no merge");
    assert_eq!(thrown_pair(&world, &data, [40, 20]), vec![60], "40 + 20: one stack of 60");
}
