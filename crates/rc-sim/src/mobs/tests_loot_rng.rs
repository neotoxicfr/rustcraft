//! Mob loot and random: imposed death table (`DeathLootTable`, seed), chicken jockey
//! (save, laying), laying through the `gameplay/chicken_lay` table, sheared wool (one entity per
//! unit, starting velocity of a dropped item).

use std::time::Duration;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use super::mob::Mob;
use super::species::Kind;
use super::tests::{drain_until, inject, kind_id, mob_nbt, move_player, prep};
use crate::tests::{start_test, test_world};

/// `DeathLootTable`, `DeathLootTableSeed` and `IsChickenJockey` read then rewritten; table and
/// seed absent: nothing written.
#[test]
fn mob_nbt_keeps_death_loot_and_jockey() {
    let mut z = Mob::new(Kind::Zombie, 1, 1, [0.0; 3], 0.0, super::jrand::JRand::new(1));
    z.load(&mob_nbt(
        "zombie",
        [0.0; 3],
        vec![
            ("DeathLootTable".into(), Nbt::String("entities/chicken".into())),
            ("DeathLootTableSeed".into(), Nbt::Long(42)),
        ],
    ));
    assert_eq!(z.death_loot.as_deref(), Some("minecraft:entities/chicken"));
    assert_eq!(z.death_loot_seed, 42);
    let saved = z.save();
    assert_eq!(saved.get("DeathLootTable").and_then(Nbt::as_str), Some("minecraft:entities/chicken"));
    assert_eq!(saved.get("DeathLootTableSeed").and_then(Nbt::as_i64), Some(42));
    z.death_loot = None;
    z.death_loot_seed = 0;
    let saved = z.save();
    assert!(saved.get("DeathLootTable").is_none() && saved.get("DeathLootTableSeed").is_none());
    let mut c = Mob::new(Kind::Chicken, 2, 2, [0.0; 3], 0.0, super::jrand::JRand::new(2));
    c.load(&mob_nbt("chicken", [0.0; 3], vec![("IsChickenJockey".into(), Nbt::Byte(1))]));
    assert!(c.chicken_jockey);
    assert_eq!(c.save().get("IsChickenJockey").and_then(|v| v.as_i64()), Some(1));
}

/// Test world, commands allowed, player at (210.5, −59, 210.5), injected entities.
fn start(
    tag: &str,
    items: &[(&str, i32)],
    ents: &[Nbt],
) -> (std::path::PathBuf, rc_client::Session, crate::Handle, std::sync::Arc<rc_data::VersionData>) {
    let (world, data) = (test_world().unwrap(), rc_testdata::version_data().unwrap());
    let dir = prep(&world, tag, 0, "normal", items);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    move_player(&dir, [210.5, -59.0, 210.5]);
    inject(&dir, ents);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    (dir, s, handle, data)
}

fn item_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:item").and_then(|r| r.entries.iter().position(|e| e == name)).map_or(-1, |i| i as i32)
}

/// Announced dropped item: entity identifier, position, velocity, item.
type Dropped = (i32, [f64; 3], [f64; 3], Option<i32>);

/// Dropped items announced during `secs`.
fn dropped(s: &rc_client::Session, data: &rc_data::VersionData, secs: u64) -> Vec<Dropped> {
    let item = kind_id(data, "minecraft:item");
    let mut out: Vec<Dropped> = Vec::new();
    drain_until(s, secs, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == item => out.push((a.id, a.pos, a.velocity, None)),
            Event::Entity(EntityEvent::Data { id, values }) => {
                for (k, v) in values {
                    if let (8, rc_proto_v777::entity::Meta::Item(Some(st))) = (k, v)
                        && let Some(o) = out.iter_mut().find(|o| o.0 == *id)
                    {
                        o.3 = Some(st.item);
                    }
                }
            }
            _ => {}
        }
        false
    });
    out
}

/// Zombie with a chicken `DeathLootTable`: killed, it drops raw chicken (imposed table), and
/// `/loot … kill` draws the same table.
#[test]
fn death_loot_table_overrides_species() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let extra = |t: &str| vec![("DeathLootTable".to_owned(), Nbt::String(t.into())), ("NoAI".to_owned(), Nbt::Byte(1))];
    // Second zombie: cursed sword (vanishing) and helmet, both with guaranteed chance.
    let stack = |id: &str, comps: Option<Nbt>| {
        let mut f = vec![("id".to_owned(), Nbt::String(id.into())), ("count".to_owned(), Nbt::Int(1))];
        if let Some(c) = comps {
            f.push(("components".to_owned(), c));
        }
        Nbt::Compound(f)
    };
    let curse = Nbt::Compound(vec![(
        "minecraft:enchantments".into(),
        Nbt::Compound(vec![("minecraft:vanishing_curse".into(), Nbt::Int(1))]),
    )]);
    let geared = mob_nbt(
        "zombie",
        [213.5, -59.0, 213.5],
        vec![
            ("NoAI".into(), Nbt::Byte(1)),
            (
                "equipment".into(),
                Nbt::Compound(vec![
                    ("mainhand".into(), stack("minecraft:diamond_sword", Some(curse))),
                    ("head".into(), stack("minecraft:iron_helmet", None)),
                ]),
            ),
            (
                "drop_chances".into(),
                Nbt::Compound(vec![("mainhand".into(), Nbt::Float(2.0)), ("head".into(), Nbt::Float(2.0))]),
            ),
        ],
    );
    let ents = [mob_nbt("zombie", [213.5, -59.0, 210.5], extra("minecraft:entities/chicken")), geared];
    let (dir, s, handle, data) = start("lootdeath", &[], &ents);
    std::thread::sleep(Duration::from_millis(1500));
    s.link.send_command("loot give @s kill @e[type=minecraft:zombie,limit=1]").unwrap();
    let mut gave = Vec::new();
    drain_until(&s, 2, |e| {
        if let Event::Chat { component, .. } = e {
            gave.push(crate::commands::cmds_ents::plain(component));
        }
        false
    });
    s.link.send_command("kill @e[type=minecraft:zombie]").unwrap();
    let drops = dropped(&s, &data, 3);
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let chicken = item_id(&data, "minecraft:chicken");
    let names = data.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default();
    let named: Vec<_> = drops.iter().map(|d| (d.1, d.3.and_then(|i| names.get(i as usize).cloned()))).collect();
    eprintln!("items dropped: {named:?}");
    assert!(gave.iter().any(|l| l.contains("minecraft:entities/chicken")), "/loot kill: {gave:?}");
    // The zombie burns in the sun on the test map: cooked chicken.
    let cooked = item_id(&data, "minecraft:cooked_chicken");
    assert!(drops.iter().any(|d| d.3 == Some(chicken) || d.3 == Some(cooked)), "death loot: {named:?}");
    assert!(drops.iter().any(|d| d.3 == Some(item_id(&data, "minecraft:iron_helmet"))), "helmet: {drops:?}");
    assert!(
        !drops.iter().any(|d| d.3 == Some(item_id(&data, "minecraft:diamond_sword"))),
        "cursed sword dropped: {drops:?}"
    );
}

/// Two chickens ready to lay: the jockey does not lay, the other drops its egg (table
/// `chicken_lay`, temperate variant).
#[test]
fn chicken_jockey_does_not_lay() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let ch = |z: f64, jockey: bool| {
        mob_nbt(
            "chicken",
            [213.5, -59.0, z],
            vec![
                ("EggLayTime".into(), Nbt::Int(30)),
                ("NoAI".into(), Nbt::Byte(1)),
                ("IsChickenJockey".into(), Nbt::Byte(i8::from(jockey))),
                ("variant".into(), Nbt::String("minecraft:temperate".into())),
            ],
        )
    };
    let (dir, s, handle, data) = start("lootjockey", &[], &[ch(213.5, true), ch(207.5, false)]);
    let drops = dropped(&s, &data, 5);
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let eggs: Vec<i32> =
        ["minecraft:egg", "minecraft:brown_egg", "minecraft:blue_egg"].iter().map(|n| item_id(&data, n)).collect();
    let near = |z: f64| drops.iter().filter(|d| (d.1[2] - z).abs() < 1.5).count();
    assert_eq!(near(213.5), 0, "the jockey laid: {drops:?}");
    assert_eq!(near(207.5), 1, "no egg: {drops:?}");
    assert!(drops.iter().all(|d| d.3.is_none_or(|i| eggs.contains(&i))), "{drops:?}");
}

/// Shearing: each unit of wool comes out with the starting velocity of a dropped item (0.2 upwards)
/// plus the sheep's small extra, at +1 in height.
#[test]
fn sheared_wool_jumps() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let ents = [mob_nbt("sheep", [212.5, -59.0, 210.5], vec![("NoAI".into(), Nbt::Byte(1))])];
    let (dir, s, handle, data) = start("lootshear", &[("shears", 1)], &ents);
    let sheep = kind_id(&data, "minecraft:sheep");
    let mut id = None;
    drain_until(&s, 10, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == sheep
        {
            id = Some(a.id);
            return true;
        }
        false
    });
    let id = id.expect("sheep announced");
    std::thread::sleep(Duration::from_millis(300));
    s.link.send_interact(id, 0, [0.0, 0.5, 0.0], false).unwrap();
    let drops = dropped(&s, &data, 2);
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!drops.is_empty(), "no wool");
    for d in &drops {
        assert!(d.2[1] >= 0.19 && d.2[1] <= 0.26, "vertical velocity {:?}", d.2);
        assert!((d.1[1] - -58.0).abs() < 1e-6, "height {:?}", d.1);
    }
}

/// Charged creeper exploding between two zombies and a skeleton: a single head dropped
/// (table `charged_creeper/root`, once per creeper).
#[test]
fn charged_creeper_drops_one_head() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let still = || vec![("NoAI".to_owned(), Nbt::Byte(1))];
    let ents = [
        mob_nbt(
            "creeper",
            [222.5, -59.0, 210.5],
            vec![("powered".into(), Nbt::Byte(1)), ("ignited".into(), Nbt::Byte(1)), ("Fuse".into(), Nbt::Short(20))],
        ),
        mob_nbt("zombie", [223.5, -59.0, 210.5], still()),
        mob_nbt("zombie", [221.5, -59.0, 210.5], still()),
        mob_nbt("skeleton", [222.5, -59.0, 211.5], still()),
    ];
    let (dir, s, handle, data) = start("lootskull", &[], &ents);
    let drops = dropped(&s, &data, 6);
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let heads = [item_id(&data, "minecraft:zombie_head"), item_id(&data, "minecraft:skeleton_skull")];
    let n = drops.iter().filter(|d| d.3.is_some_and(|i| heads.contains(&i))).count();
    assert_eq!(n, 1, "heads dropped: {drops:?}");
}

/// Right click, shears in hand: ripe bush picked (berries on the ground, back to age 1, table
/// `harvest/sweet_berry_bush`), cave vine picked, pumpkin carved from the west face
/// (seeds in front of it, carved pumpkin facing west, table `carve/pumpkin`).
#[test]
fn player_harvests_and_carves() {
    if test_world().is_none() || rc_testdata::version_data().is_none() {
        return;
    }
    let (dir, s, handle, data) = start("lootharvest", &[("shears", 1)], &[]);
    std::thread::sleep(Duration::from_millis(1500));
    let (bush, vine, pumpkin) = ([212, -59, 210], [212, -57, 208], [212, -59, 212]);
    for c in [
        "setblock 212 -59 210 minecraft:sweet_berry_bush[age=3]".to_owned(),
        "setblock 212 -56 208 minecraft:stone".to_owned(),
        "setblock 212 -57 208 minecraft:cave_vines[berries=true]".to_owned(),
        "setblock 212 -59 212 minecraft:pumpkin".to_owned(),
    ] {
        s.link.send_command(&c).unwrap();
    }
    let _ = dropped(&s, &data, 1);
    let state_name = |st: u32| {
        let props: Vec<String> = data.state_properties(st).map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}[{}]", data.block_of(st).map_or("?", |b| b.name.as_str()), props.join(","))
    };
    let mut seen = Vec::new();
    let mut drops = Vec::new();
    for (i, (p, face)) in [(bush, 1), (vine, 4), (pumpkin, 4)].into_iter().enumerate() {
        s.link.send_use_item_on(p, face, [0.5, 0.5, 0.5], false, 60 + i as i32).unwrap();
        let item = kind_id(&data, "minecraft:item");
        drain_until(&s, 1, |e| {
            match e {
                Event::BlockUpdate { pos, state } if [pos.x, pos.y, pos.z] == p => seen.push((p, state_name(*state))),
                Event::Entity(EntityEvent::Add(a)) if a.kind == item => drops.push((p, a.pos)),
                _ => {}
            }
            false
        });
    }
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let has = |p: [i32; 3], want: &str| seen.iter().any(|(q, n)| *q == p && n.contains(want));
    assert!(has(bush, "sweet_berry_bush[age=1]"), "{seen:?}");
    assert!(has(vine, "berries=false"), "{seen:?}");
    assert!(has(pumpkin, "carved_pumpkin[facing=west"), "{seen:?}");
    let n = |p: [i32; 3]| drops.iter().filter(|d| d.0 == p).count();
    assert!(n(bush) >= 1 && n(vine) >= 1 && n(pumpkin) >= 1, "{drops:?}");
    // Seeds in front of the west face: x = 212 + 0.5 − 0.65.
    assert!(drops.iter().any(|d| d.0 == pumpkin && (d.1[0] - 211.85).abs() < 1e-6), "{drops:?}");
}
