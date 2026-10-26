//! Family F01: unit tests (effects, conversions, save) and end to end on a
//! copy of the test world (`RUSTCRAFT_SOLO_WORLD`): husk (hunger), cave spider (poison),
//! stray (coloured arrow), zombie converting a villager (Hard), cure completed,
//! endermite at end of life, bogged shearing, phantom that stings. Absent: tests ignored.

use std::time::{Duration, Instant};

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use super::effects::{Effect, add};
use super::jrand::JRand;
use super::mob::Mob;
use super::species::{Kind, dims};
use super::tests::{inject, kind_id, mob_nbt, move_player, prep};
use super::variants::{arrow_effect, tracker_step};
use crate::tests::{start_test, test_world};

fn mob(kind: Kind) -> Mob {
    let mut m = Mob::new(kind, 1, 7, [0.5, 0.0, 0.5], 0.0, JRand::new(42));
    m.define_data();
    m
}

#[test]
fn bases_and_dimensions() {
    assert_eq!(Kind::Husk.base(), Kind::Zombie);
    assert_eq!(Kind::ZombieVillager.base(), Kind::Zombie);
    assert_eq!(Kind::Parched.base(), Kind::Skeleton);
    assert_eq!(Kind::CaveSpider.base(), Kind::Spider);
    assert_eq!(Kind::Phantom.base(), Kind::Phantom);
    assert!(Kind::Stray.undead() && Kind::Phantom.undead() && !Kind::Giant.undead());
    assert_eq!(dims(Kind::Husk, true), (0.49, 0.98, 0.825));
    assert_eq!(dims(Kind::ZombieVillager, true), (0.49, 0.98, 0.67));
    assert_eq!(dims(Kind::Giant, false), (3.6, 12.0, 10.44));
    assert!(!Kind::Husk.info().burns && !Kind::Parched.info().burns);
    assert!(Kind::Stray.info().burns && Kind::Bogged.info().burns && Kind::Phantom.info().burns);
    assert_eq!(Kind::Bogged.info().health, 16.0);
    assert_eq!(Kind::CaveSpider.info().health, 12.0);
    assert_eq!(Kind::Endermite.info().xp, 3);
    assert_eq!(arrow_effect(Kind::Stray), Some(("minecraft:slowness", 600)));
    assert_eq!(arrow_effect(Kind::Skeleton), None);
    // Size-4 phantom: dimensions × 1.6.
    let mut p = mob(Kind::Phantom);
    p.x.size = 4;
    assert!((p.width() - 0.9 * 1.6).abs() < 1e-6 && (p.height() - 0.5 * 1.6).abs() < 1e-6);
}

#[test]
fn effect_update_rules() {
    let mut l = Vec::new();
    assert!(add(&mut l, Effect::new("minecraft:poison", 100, 0)).is_some());
    // Same level, shorter: nothing.
    assert!(add(&mut l, Effect::new("minecraft:poison", 50, 0)).is_none());
    // Same level, longer: duration replaced.
    assert_eq!(add(&mut l, Effect::new("minecraft:poison", 300, 0)).map(|e| e.dur), Some(300));
    // Stronger and shorter: the old one becomes hidden.
    let e = add(&mut l, Effect::new("minecraft:poison", 40, 1)).expect("changed");
    assert_eq!((e.amp, e.dur), (1, 40));
    assert_eq!(e.hidden.as_ref().map(|h| (h.amp, h.dur)), Some((0, 300)));
    // Weaker and longer: hidden updated, visible unchanged.
    assert!(add(&mut l, Effect::new("minecraft:poison", 600, 0)).is_none());
    assert_eq!(l[0].hidden.as_ref().map(|h| h.dur), Some(600));
    assert_eq!(l.len(), 1);
    // NBT round trip.
    let back = Effect::from_nbt(&l[0].to_nbt()).expect("lu");
    assert_eq!(back, l[0]);
    assert!(!super::effects::can_affect(Kind::CaveSpider, "minecraft:poison"));
    assert!(!super::effects::can_affect(Kind::Husk, "minecraft:poison"));
    assert!(super::effects::can_affect(Kind::Silverfish, "minecraft:poison"));
    assert!(!super::effects::can_affect(Kind::Parched, "minecraft:weakness"));
}

/// Drowning: 600 ticks of affliction then 300 of conversion; leaving the water resets to −1.
#[test]
fn drowning_tracker_timing() {
    let mut m = mob(Kind::Husk);
    let mut n = 0;
    while !tracker_step(&mut m, true, 600) {
        n += 1;
        assert!(n < 2000);
    }
    // Start at 0: 600 ticks of affliction, then 301 decrements (end when < 0).
    assert_eq!(n, 600 + 300);
    let mut m = mob(Kind::Zombie);
    for _ in 0..10 {
        tracker_step(&mut m, true, 600);
    }
    tracker_step(&mut m, false, 600);
    assert_eq!((m.x.afflict, m.x.conv_time), (-1, -1));
    let mut n = 0;
    while !tracker_step(&mut m, true, 600) {
        n += 1;
    }
    assert_eq!(n, 601 + 300);
}

/// Save and re-read of the specific fields (zombie villager, phantom, bogged, endermite).
#[test]
fn extra_nbt_roundtrip() {
    let mut z = mob(Kind::ZombieVillager);
    z.villager = [0, 9, 3];
    z.x.vdata_final = true;
    z.x.cure_time = 1234;
    z.x.cure_starter = Some(0xABCD);
    z.x.villager_xp = 17;
    z.x.effects.push(Effect::new("minecraft:weakness", 200, 0));
    let e = z.save();
    let vd = e.get("VillagerData").expect("data");
    assert_eq!(vd.get("profession").and_then(Nbt::as_str), Some("minecraft:librarian"));
    assert_eq!(vd.get("type").and_then(Nbt::as_str), Some("minecraft:desert"));
    let mut r = Mob::new(Kind::ZombieVillager, 2, 0, [0.0; 3], 0.0, JRand::new(1));
    r.load(&e);
    r.define_data();
    assert_eq!(r.villager, [0, 9, 3]);
    assert_eq!((r.x.cure_time, r.x.cure_starter, r.x.villager_xp), (1234, Some(0xABCD), 17));
    assert_eq!(r.x.effects.len(), 1);
    let mut p = mob(Kind::Phantom);
    p.x.size = 3;
    p.x.anchor = Some([1, 70, -2]);
    let mut q = Mob::new(Kind::Phantom, 3, 0, [0.0; 3], 0.0, JRand::new(1));
    q.load(&p.save());
    assert_eq!((q.x.size, q.x.anchor), (3, Some([1, 70, -2])));
    let mut b = mob(Kind::Bogged);
    b.sheared = true;
    let mut c = Mob::new(Kind::Bogged, 4, 0, [0.0; 3], 0.0, JRand::new(1));
    c.load(&b.save());
    assert!(c.sheared);
    let mut m = mob(Kind::Endermite);
    m.x.life = 2000;
    let mut n = Mob::new(Kind::Endermite, 5, 0, [0.0; 3], 0.0, JRand::new(1));
    n.load(&m.save());
    assert_eq!(n.x.life, 2000);
}

/// Villager ↔ zombie villager: experience, offers and reputation follow the conversion
/// (a villager that has traded does not become a zombie that despawns far away).
#[test]
fn trade_data_follows_conversion() {
    let offers = Nbt::Compound(vec![("Recipes".into(), Nbt::List(Vec::new()))]);
    let gossips = Nbt::List(Vec::new());
    let mut src = Nbt::Compound(Vec::new());
    crate::nbtio::set(&mut src, "id", Nbt::String("minecraft:villager".into()));
    crate::nbtio::set(&mut src, "Xp", Nbt::Int(42));
    crate::nbtio::set(&mut src, "Offers", offers.clone());
    crate::nbtio::set(&mut src, "Gossips", gossips.clone());
    let mut v = Mob::new(Kind::Villager, 1, 0, [0.0; 3], 0.0, JRand::new(1));
    v.load(&src);
    assert_eq!(v.x.villager_xp, 42);
    let mut z = mob(Kind::ZombieVillager);
    crate::server::Server::copy_trade_data(&v, &mut z);
    let e = z.save();
    assert_eq!(e.get("Xp").and_then(Nbt::as_i64), Some(42));
    assert_eq!(e.get("Offers"), Some(&offers));
    assert_eq!(e.get("Gossips"), Some(&gossips));
    // Back to the cure.
    let mut back = mob(Kind::Villager);
    crate::server::Server::copy_trade_data(&z, &mut back);
    let e = back.save();
    assert_eq!(e.get("Xp").and_then(Nbt::as_i64), Some(42));
    assert_eq!(e.get("Offers"), Some(&offers));
}

// ---------- End to end ----------

fn wait_for(s: &rc_client::Session, secs: u64, mut f: impl FnMut(&Event) -> bool) -> bool {
    crate::tests::wait_game(s, secs, &mut f)
}

fn effect_id(data: &rc_data::VersionData, name: &str) -> i32 {
    data.registry("minecraft:mob_effect").unwrap().entries.iter().position(|e| e == name).unwrap() as i32
}

/// Scenario: mobs injected near the player (without AI or not), once the server is started.
fn scenario(tag: &str, mode: i32, diff: &str, items: &[(&str, i32)], ents: &[Nbt]) -> std::path::PathBuf {
    let world = test_world().expect("world");
    let dir = prep(&world, tag, mode, diff, items);
    move_player(&dir, [210.5, -59.0, 210.5]);
    inject(&dir, ents);
    dir
}

/// Husk (empty hand) that hits: hunger effect received by the player (Normal: 140 × ⌊difficulty⌋).
#[test]
fn husk_hunger_on_hit() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = scenario("f01husk", 0, "normal", &[], &[mob_nbt("husk", [212.5, -59.0, 210.5], vec![])]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let hunger = effect_id(&data, "minecraft:hunger");
    let mut got = None;
    wait_for(&s, 20, |e| match e {
        Event::Entity(EntityEvent::Effect { effect, duration, .. }) if *effect == hunger => {
            got = Some(*duration);
            true
        }
        _ => false,
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("hunger: {got:?}");
    assert!(got.is_some_and(|d| d > 0 && d % 140 == 0), "no hunger ({got:?})");
}

/// Cave spider: poison 7 s (140 ticks) in Normal.
#[test]
fn cave_spider_poison() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = scenario("f01cave", 0, "normal", &[], &[mob_nbt("cave_spider", [212.5, -59.0, 210.5], vec![])]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let poison = effect_id(&data, "minecraft:poison");
    let spider = kind_id(&data, "minecraft:cave_spider");
    // By day the spider is neutral: we hit it so that it retaliates.
    let mut sid = None;
    wait_for(&s, 10, |e| match e {
        Event::Entity(EntityEvent::Add(a)) if a.kind == spider => {
            sid = Some(a.id);
            true
        }
        _ => false,
    });
    std::thread::sleep(Duration::from_millis(500));
    s.link.send_attack(sid.expect("spider")).unwrap();
    let mut got = None;
    wait_for(&s, 20, |e| match e {
        Event::Entity(EntityEvent::Effect { effect, duration, .. }) if *effect == poison => {
            got = Some(*duration);
            true
        }
        _ => false,
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got, Some(140), "poison {got:?}");
}

/// Stray: arrow announced with the slowness colour (metadata 11), slowness to the player.
#[test]
fn stray_slowness_arrow() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let stack = |id: &str| Nbt::Compound(vec![("id".into(), Nbt::String(id.into())), ("count".into(), Nbt::Int(1))]);
    let eq = Nbt::Compound(vec![
        ("mainhand".into(), stack("minecraft:bow")),
        ("head".into(), stack("minecraft:leather_helmet")),
    ]);
    let dir = scenario(
        "f01stray",
        0,
        "normal",
        &[],
        &[mob_nbt("stray", [218.5, -59.0, 210.5], vec![("equipment".into(), eq)])],
    );
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let arrow = kind_id(&data, "minecraft:arrow");
    let slow = effect_id(&data, "minecraft:slowness");
    let (mut arrow_id, mut color, mut effect) = (None, None, None);
    wait_for(&s, 30, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == arrow => arrow_id = Some(a.id),
            Event::Entity(EntityEvent::Data { id, values }) if Some(*id) == arrow_id => {
                for (i, v) in values {
                    if *i == 11
                        && let rc_proto_v777::entity::Meta::Int(c) = v
                    {
                        color = Some(*c);
                    }
                }
            }
            Event::Entity(EntityEvent::Effect { effect: ef, duration, .. }) if *ef == slow => {
                effect = Some(*duration);
            }
            _ => {}
        }
        color.is_some() && effect.is_some()
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("arrow {arrow_id:?} colour {color:?} slowness {effect:?}");
    assert_eq!(color, Some((0xFF00_0000u32 | 9_154_528) as i32));
    assert_eq!(effect, Some(600));
}

/// Hard: a zombie kills a villager without AI → zombie villager (same profession).
#[test]
fn zombie_converts_villager() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let vd = Nbt::Compound(vec![
        ("level".into(), Nbt::Int(2)),
        ("profession".into(), Nbt::String("minecraft:farmer".into())),
        ("type".into(), Nbt::String("minecraft:plains".into())),
    ]);
    let ents = [
        mob_nbt("villager", [215.5, -59.0, 214.5], vec![("NoAI".into(), Nbt::Byte(1)), ("VillagerData".into(), vd)]),
        mob_nbt("zombie", [218.5, -59.0, 214.5], vec![]),
    ];
    // Creative: the player is not a target, the zombie goes to the villager.
    let dir = scenario("f01conv", 1, "hard", &[], &ents);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let zv = kind_id(&data, "minecraft:zombie_villager");
    let mut zv_id = None;
    let mut farmer = false;
    let farmer_id = super::variants::PROFESSIONS.iter().position(|p| *p == "farmer").unwrap() as i32;
    wait_for(&s, 40, |e| {
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == zv => zv_id = Some(a.id),
            Event::Entity(EntityEvent::Data { id, values }) if Some(*id) == zv_id => {
                farmer |= values.iter().any(|(i, v)| {
                    *i == 20 && matches!(v, rc_proto_v777::entity::Meta::Villager([_, p, 2]) if *p == farmer_id)
                });
            }
            _ => {}
        }
        farmer
    });
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(zv_id.is_some(), "no zombie villager");
    assert!(farmer, "profession not kept");
}

/// Cure almost finished (ConversionTime 10): becomes a villager; endermite at end of life
/// removed; bogged sheared (items dropped, flag 16).
#[test]
fn cure_endermite_and_shear() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let ents = [
        mob_nbt(
            "zombie_villager",
            [214.5, -59.0, 206.5],
            vec![("NoAI".into(), Nbt::Byte(1)), ("ConversionTime".into(), Nbt::Int(10))],
        ),
        mob_nbt(
            "endermite",
            [206.5, -59.0, 206.5],
            vec![("NoAI".into(), Nbt::Byte(1)), ("Lifetime".into(), Nbt::Int(2390))],
        ),
        mob_nbt("bogged", [212.5, -59.0, 210.5], vec![("NoAI".into(), Nbt::Byte(1))]),
    ];
    // PersistenceRequired of mob_nbt blocks the endermite's ageing: we remove it.
    let ents: Vec<Nbt> = ents
        .into_iter()
        .map(|mut e| {
            if e.get("id").and_then(Nbt::as_str) == Some("minecraft:endermite") {
                crate::nbtio::remove(&mut e, "PersistenceRequired");
            }
            e
        })
        .collect();
    let dir = scenario("f01cure", 0, "normal", &[("shears", 1)], &ents);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let (villager, mite, bogged, item) = (
        kind_id(&data, "minecraft:villager"),
        kind_id(&data, "minecraft:endermite"),
        kind_id(&data, "minecraft:bogged"),
        kind_id(&data, "minecraft:item"),
    );
    let (mut cured, mut mite_id, mut mite_gone, mut bog, mut items, mut sheared) = (false, None, false, None, 0, false);
    s.link.send_carried_item(0).unwrap();
    let t = Instant::now();
    let mut asked = false;
    while t.elapsed() < Duration::from_secs(15) {
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        match e {
            Event::Entity(EntityEvent::Add(a)) if a.kind == villager => cured = true,
            Event::Entity(EntityEvent::Add(a)) if a.kind == mite => mite_id = Some(a.id),
            Event::Entity(EntityEvent::Add(a)) if a.kind == bogged => bog = Some(a.id),
            Event::Entity(EntityEvent::Add(a)) if a.kind == item => items += 1,
            Event::Entity(EntityEvent::Remove(l)) if mite_id.is_some_and(|m| l.contains(&m)) => mite_gone = true,
            Event::Entity(EntityEvent::Data { id, values }) if Some(id) == bog => {
                sheared |= values.iter().any(|(i, v)| *i == 16 && *v == rc_proto_v777::entity::Meta::Bool(true));
            }
            _ => {}
        }
        if let Some(b) = bog
            && !asked
            && t.elapsed() > Duration::from_secs(2)
        {
            asked = true;
            s.link.send_interact(b, 0, [0.0, 1.0, 0.0], false).unwrap();
        }
        if cured && mite_gone && sheared && items > 0 {
            break;
        }
    }
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("cured {cured}, endermite removed {mite_gone}, sheared {sheared}, items {items}");
    assert!(cured, "cure not completed");
    assert!(mite_gone, "endermite not removed");
    assert!(sheared && items > 0, "shearing");
}

/// Phantom injected above the player: it stings and hits them.
#[test]
fn phantom_swoops() {
    let (Some(_), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    // Helmet: by day, a burning (hurt) phantom abandons its dives, like in the game.
    let helmet = Nbt::Compound(vec![(
        "head".into(),
        Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:leather_helmet".into())),
            ("count".into(), Nbt::Int(1)),
        ]),
    )]);
    let dir = scenario(
        "f01phantom",
        0,
        "normal",
        &[],
        &[mob_nbt("phantom", [214.5, -45.0, 210.5], vec![("equipment".into(), helmet)])],
    );
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    let hit = wait_for(&s, 30, |e| matches!(e, Event::Health { health, .. } if *health < 20.0));
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(hit, "the phantom did not hit");
}

/// Demonstration world (`RUSTCRAFT_F01_PREP=<folder>`, `RUSTCRAFT_F01_SCENARIO=show|fight`):
/// `show` lines up the ten species without AI in front of the player (creative); `fight` puts a husk and
/// a cave spider in contact (survival, Normal) to see the effects in the HUD.
#[test]
#[ignore]
fn prep_f01_world() {
    let (Ok(dir), Ok(sc)) = (std::env::var("RUSTCRAFT_F01_PREP"), std::env::var("RUSTCRAFT_F01_SCENARIO")) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let tmp = dir.with_extension("src");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::rename(&dir, &tmp).unwrap();
    let helmet = || {
        Nbt::Compound(vec![(
            "head".into(),
            Nbt::Compound(vec![
                ("id".into(), Nbt::String("minecraft:chainmail_helmet".into())),
                ("count".into(), Nbt::Int(1)),
            ]),
        )])
    };
    let rot = |yaw: f32| Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(0.0)]);
    let still = |id: &str, p: [f64; 3], extra: Vec<(String, Nbt)>| {
        let mut v =
            vec![("NoAI".into(), Nbt::Byte(1)), ("Rotation".into(), rot(180.0)), ("equipment".into(), helmet())];
        v.extend(extra);
        mob_nbt(id, p, v)
    };
    let (mode, ents): (i32, Vec<Nbt>) = match sc.as_str() {
        "fight" => (
            0,
            vec![mob_nbt("husk", [211.5, -59.0, 212.5], vec![]), mob_nbt("cave_spider", [209.5, -59.0, 212.5], vec![])],
        ),
        _ => (
            1,
            vec![
                still("husk", [205.5, -59.0, 217.5], vec![]),
                still("zombie_villager", [207.5, -59.0, 217.5], vec![]),
                still("stray", [209.5, -59.0, 217.5], vec![]),
                still("bogged", [211.5, -59.0, 217.5], vec![]),
                still("parched", [213.5, -59.0, 217.5], vec![]),
                still("cave_spider", [215.5, -59.0, 217.5], vec![]),
                still("silverfish", [206.5, -59.0, 214.5], vec![]),
                still("endermite", [208.5, -59.0, 214.5], vec![]),
                still("phantom", [212.5, -56.0, 214.5], vec![("size".into(), Nbt::Int(2))]),
                still("bogged", [214.5, -59.0, 214.5], vec![("sheared".into(), Nbt::Byte(1))]),
                still("giant", [210.5, -59.0, 236.5], vec![]),
            ],
        ),
    };
    let made = prep(&tmp, "f01prep", mode, "normal", &[]);
    move_player(&made, [210.5, -59.0, 208.5]);
    for e in std::fs::read_dir(made.join("players").join("data")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("dat") {
            let mut pl = crate::nbtio::read_dat(&p).unwrap();
            crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(-8.0)]));
            std::fs::write(&p, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
        }
    }
    inject(&made, &ents);
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::rename(&made, &dir).unwrap();
    eprintln!("world prepared: {}", dir.display());
}
