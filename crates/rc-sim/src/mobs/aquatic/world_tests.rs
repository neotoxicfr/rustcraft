//! Drowned and guardians end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`,
//! area 200..231) driven by the probe: guardian swimming and beam, elder guardian fatigue,
//! drowned that swims toward the player at night, that reaches the water by day, that throws its trident,
//! converted drowned zombie, monument spawn list.

use rc_proto::nbt::Nbt;

use super::super::mob::Ent;
use super::super::species::{Category, Kind};
use crate::blocks::probe::{Probe, state_string};

/// Stone floor at −60, air above, survival player (normal difficulty).
fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {:?} or data missing: test skipped", world);
        return None;
    };
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -50 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("area 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.view = 8;
    // "Loaded" player: can take damage from the first tick.
    p.s.loaded = true;
    Some((p, dir))
}

fn step(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.tick();
        p.s.mobs_tick();
        p.s.projectiles_tick(); // mob projectiles: common engine (S8)
    }
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let m = p.s.new_mob(k, pos, 0.0);
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// Time of day of the overworld clock (6000 noon, 18000 midnight).
fn set_day_time(p: &mut Probe, t: i64) {
    match p.s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
        Some(c) => c.1 = t,
        None => p.s.clocks.push(("minecraft:overworld".into(), t)),
    }
    p.s.time = t;
}

fn mob(p: &Probe, id: i32) -> Option<&super::super::mob::Mob> {
    p.s.mob_ref(id)
}

/// Guardian in a pool: it swims (swim navigation), aims at the player, locks the beam
/// after 10 ticks (index 17) and hurts them after 80.
#[test]
fn guardian_swims_and_beams() {
    let Some((mut p, dir)) = probe("f08guard") else { return };
    p.run("fill 200 -59 200 231 -53 231 minecraft:water\nplayer 222.5 -59 216.5 90 0\n");
    let g = spawn(&mut p, Kind::Guardian, [208.5, -57.0, 216.5]);
    let start = p.s.life.vit.health;
    let (mut moved, mut locked, mut hit) = (false, None, None);
    for t in 0..400 {
        step(&mut p, 1);
        let Some(m) = mob(&p, g) else { break };
        if std::env::var("F08_DEBUG").is_ok() && t < 120 {
            let running: Vec<i32> = m.goals.goals.iter().filter(|w| w.running).map(|w| w.prio).collect();
            eprintln!("t{t} target {:?} beam {:?} goals {running:?} pos {:?}", m.target, m.aqua.beam, m.pos);
        }
        moved |= m.aqua.moving;
        if locked.is_none() && m.aqua.beam.is_some() {
            locked = Some(t);
        }
        if hit.is_none() && p.s.life.vit.health < start {
            hit = Some(t);
            break;
        }
    }
    let m = mob(&p, g).unwrap();
    eprintln!("moved {moved}, lock {locked:?}, hit {hit:?}, pos {:?}, HP {}", m.pos, p.s.life.vit.health);
    assert!(m.in_water, "the guardian left the water");
    assert!(locked.is_some(), "beam never locked");
    let (l, h) = (locked.unwrap(), hit.expect("beam without damage"));
    assert_eq!(h - l, 80, "damage 80 ticks after the lock");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Swim path: only water cells, around a pillar.
#[test]
fn guardian_swim_path_stays_in_water() {
    let Some((mut p, dir)) = probe("f08path") else { return };
    p.run("fill 200 -59 200 231 -53 231 minecraft:water\nfill 210 -59 214 212 -53 218 minecraft:stone\n");
    let g = spawn(&mut p, Kind::Guardian, [206.5, -57.0, 216.5]);
    let i = p.s.mob_index(g).unwrap();
    let mut m = p.s.mobs.list[i].take().unwrap();
    m.in_water = true;
    let path = p.s.create_path(&mut m, [214, -57, 216], 1).expect("swim path");
    p.s.mobs.list[i] = Some(m);
    eprintln!("nodes {:?}", path.nodes.iter().map(|n| n.0).collect::<Vec<_>>());
    assert!(path.reached, "target not reached");
    for (n, _) in &path.nodes {
        let s = state_string(&p.s, p.s.get(*n));
        assert!(s.starts_with("minecraft:water"), "{n:?} out of the water: {s}");
    }
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Elder guardian: mining fatigue III (6000 ticks) to the survival player within 50 blocks,
/// residence zone set at its position (radius 16).
#[test]
fn elder_guardian_mining_fatigue() {
    let Some((mut p, dir)) = probe("f08elder") else { return };
    p.run("fill 200 -59 200 231 -53 231 minecraft:water\n");
    let e = spawn(&mut p, Kind::ElderGuardian, [210.5, -58.0, 216.5]);
    // Next multiple of 1200 in a few ticks.
    if let Some(i) = p.s.mob_index(e)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.tick_count = 1200 - m.id - 3;
    }
    step(&mut p, 6);
    let fx = p.s.fx().get("minecraft:mining_fatigue").cloned();
    let m = mob(&p, e).unwrap();
    eprintln!("effect {fx:?}, home {:?}", m.aqua.home);
    let fx = fx.expect("no mining fatigue");
    assert_eq!(fx.amp, 2);
    assert!(fx.dur > 5990 && fx.dur <= 6000);
    assert_eq!(m.aqua.home.map(|h| h.1), Some(16));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Drowned at night: the player is in the water, it swims (shared flag) and approaches.
#[test]
fn drowned_swims_to_player_at_night() {
    let Some((mut p, dir)) = probe("f08swim") else { return };
    set_day_time(&mut p, 18000);
    p.run("fill 200 -59 200 231 -51 231 minecraft:water\nplayer 220.5 -54 216.5 90 0\n");
    let d = spawn(&mut p, Kind::Drowned, [208.5, -59.0, 216.5]);
    let d0 = super::super::mth::dist2(mob(&p, d).unwrap().pos, p.s.pos);
    let mut swam = false;
    let mut target = false;
    for _ in 0..300 {
        step(&mut p, 1);
        let Some(m) = mob(&p, d) else { break };
        swam |= m.aqua.swimming;
        target |= m.target == Some(Ent::Player);
    }
    let m = mob(&p, d).unwrap();
    let d1 = super::super::mth::dist2(m.pos, p.s.pos);
    eprintln!("target {target}, swim {swam}, distance² {d0:.1} → {d1:.1}, pos {:?}", m.pos);
    assert!(target, "the drowned does not target the player in the water");
    assert!(swam, "the drowned never swims");
    assert!(d1 < d0 * 0.5, "the drowned does not approach");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Drowned by day on dry land: it reaches the nearby water.
#[test]
fn drowned_goes_to_water_by_day() {
    let Some((mut p, dir)) = probe("f08water") else { return };
    set_day_time(&mut p, 6000);
    p.run("fill 214 -59 210 222 -57 222 minecraft:water\nplayer 230.5 -59 230.5 0 0\n");
    let d = spawn(&mut p, Kind::Drowned, [206.5, -59.0, 216.5]);
    if let Some(i) = p.s.mob_index(d)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        // Helmet: no burning in sunlight during the test.
        m.equipment[5] =
            Some(crate::player::Stack { id: "minecraft:leather_helmet".into(), count: 1, components: None });
    }
    let mut reached = None;
    for t in 0..600 {
        step(&mut p, 1);
        if mob(&p, d).is_some_and(|m| m.in_water) {
            reached = Some(t);
            break;
        }
    }
    eprintln!("water reached {reached:?}, pos {:?}", mob(&p, d).map(|m| m.pos));
    assert!(reached.is_some(), "the drowned does not reach the water");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Drowned armed with a trident, at night: it throws (`trident` projectile), the player is hit.
#[test]
fn drowned_throws_trident() {
    let Some((mut p, dir)) = probe("f08trident") else { return };
    set_day_time(&mut p, 18000);
    p.run("player 216.5 -59 216.5 90 0\n");
    let d = spawn(&mut p, Kind::Drowned, [208.5, -59.0, 216.5]);
    if let Some(i) = p.s.mob_index(d)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.equipment[0] = Some(crate::player::Stack { id: "minecraft:trident".into(), count: 1, components: None });
    }
    let start = p.s.life.vit.health;
    let (mut thrown, mut hurt) = (0, false);
    let mut seen = hashbrown::HashSet::new();
    for _ in 0..300 {
        step(&mut p, 1);
        for t in p.s.proj.list.iter().filter(|t| t.kind == crate::projectiles::PKind::Trident) {
            seen.insert(t.id);
            if std::env::var("F08_DEBUG").is_ok() && seen.len() <= 1 {
                eprintln!("trident {} pos {:?} v {:?} ground {} damage {}", t.id, t.pos, t.vel, t.in_ground, t.dealt_damage);
            }
        }
        thrown = seen.len();
        hurt |= p.s.life.vit.health < start;
        if thrown >= 1 && hurt {
            break;
        }
    }
    let m = mob(&p, d).unwrap();
    eprintln!("tridents {thrown}, hurt {hurt}, HP {}, aggressive {}", p.s.life.vit.health, m.aggressive);
    assert!(thrown >= 1, "no trident thrown");
    assert!(hurt, "player never hit");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Zombie underwater: 600 ticks drowned then 300 of conversion → drowned (event 1040).
#[test]
fn zombie_converts_to_drowned() {
    let Some((mut p, dir)) = probe("f08conv") else { return };
    p.run("fill 214 -59 214 218 -55 218 minecraft:stone\nfill 215 -59 215 217 -55 217 minecraft:water\nplayer 200.5 -59 200.5 0 0\n");
    let z = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 216.5]);
    let mut started = None;
    for t in 0..1000 {
        step(&mut p, 1);
        if started.is_none() && mob(&p, z).is_some_and(|m| m.x.conv_time >= 0) {
            started = Some(t);
        }
        if mob(&p, z).is_none() {
            break;
        }
    }
    let drowned = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Drowned).count();
    eprintln!("conversion started {started:?}, drowned {drowned}");
    assert!(started.is_some_and(|t| (595..=605).contains(&t)), "conversion started at {started:?}");
    assert_eq!(drowned, 1, "zombie not converted into a drowned");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Monument: the "monsters" list at a point of its box is the structure's
/// (guardians), outside the box that of the biome.
#[test]
fn monument_overrides_monster_list() {
    let Some((mut p, dir)) = probe("f08monu") else { return };
    let (cx, cz) = (13, 13);
    let key = (i64::from(cz) << 32) | (i64::from(cx) & 0xFFFF_FFFF);
    let start = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:monument".into())),
        ("ChunkX".into(), Nbt::Int(cx)),
        ("ChunkZ".into(), Nbt::Int(cz)),
        (
            "Children".into(),
            Nbt::List(vec![Nbt::Compound(vec![
                ("id".into(), Nbt::String("minecraft:omb".into())),
                ("BB".into(), Nbt::IntArray(vec![200, -63, 200, 215, -40, 215])),
            ])]),
        ),
    ]);
    let structures = Nbt::Compound(vec![
        ("References".into(), Nbt::Compound(vec![("minecraft:monument".into(), Nbt::LongArray(vec![key]))])),
        ("starts".into(), Nbt::Compound(vec![("minecraft:monument".into(), start)])),
    ]);
    if let Some(Some(c)) = p.s.chunks.get_mut(&(cx, cz)) {
        crate::nbtio::set(&mut c.root, "structures", structures);
    } else {
        eprintln!("chunk (13, 13) missing");
        return;
    }
    // List at the position (F07 `spawn_list_at`): monument in its box, biome elsewhere.
    let guardian = |l: &[serde_json::Value]| l.iter().any(|e| e["type"].as_str() == Some("minecraft:guardian"));
    let inside = p.s.spawn_list_at(Category::Monster, "monster", [208, -50, 208]);
    let outside = p.s.spawn_list_at(Category::Monster, "monster", [208, -30, 208]);
    eprintln!("inside {inside:?}, outside {outside:?}");
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0]["type"].as_str(), Some("minecraft:guardian"));
    assert!(!guardian(&outside));
    assert!(!guardian(&p.s.spawn_list_at(Category::Creature, "creature", [208, -50, 208])));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save: drowned (baby, trident, shell dropped for sure), elder guardian (residence,
/// persistence) read back.
#[test]
fn aquatic_saved_and_reloaded() {
    let Some((mut p, dir)) = probe("f08save") else { return };
    p.run("fill 200 -59 200 231 -53 231 minecraft:water\n");
    let d = spawn(&mut p, Kind::Drowned, [210.5, -59.0, 210.5]);
    if let Some(i) = p.s.mob_index(d)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.baby = true;
        m.equipment[0] = Some(crate::player::Stack { id: "minecraft:trident".into(), count: 1, components: None });
        m.equipment[1] =
            Some(crate::player::Stack { id: "minecraft:nautilus_shell".into(), count: 1, components: None });
        m.drop_chance[1] = 2.0;
        m.no_ai = true;
    }
    let e = spawn(&mut p, Kind::ElderGuardian, [212.5, -58.0, 210.5]);
    if let Some(i) = p.s.mob_index(e)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.aqua.home = Some(([212, -58, 210], 16));
        m.no_ai = true;
    }
    step(&mut p, 2);
    p.run("save\n");
    let list = super::super::tests::saved(&dir, 210 >> 4, 210 >> 4);
    let dn = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:drowned"));
    let el = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:elder_guardian"));
    eprintln!("drowned {dn:?}\nelder {el:?}");
    let dn = dn.expect("drowned saved");
    assert_eq!(crate::nbtio::int(dn, "IsBaby"), Some(1));
    assert_eq!(crate::nbtio::int(dn, "DrownedConversionTime"), Some(-1));
    // Shell dropped for sure: chance kept at save.
    assert_eq!(dn.get("drop_chances"), Some(&Nbt::Compound(vec![("offhand".into(), Nbt::Float(2.0))])));
    let mut back = p.s.new_mob(Kind::Drowned, [0.0; 3], 0.0);
    back.load(dn);
    assert_eq!(back.drop_chance[1], 2.0);
    assert_eq!(back.drop_chance[0], 0.085);
    let el = el.expect("elder guardian saved");
    assert_eq!(crate::nbtio::int(el, "home_radius"), Some(16));
    assert_eq!(el.get("home_pos"), Some(&Nbt::IntArray(vec![212, -58, 210])));
    assert_eq!(crate::nbtio::int(el, "PersistenceRequired"), Some(1));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Demo world (screenshots): pool, guardian, elder guardian and drowned in front of the
/// player on the bank. `RUSTCRAFT_F08_PREP` = destination, `RUSTCRAFT_F08_SCENE` = `static`
/// (no AI, creative) or `live` (AI, survival: beam and swimming toward the player in the water).
#[test]
#[ignore]
fn prep_aquatic_world() {
    let (Ok(dest), Some(world), Some(data)) =
        (std::env::var("RUSTCRAFT_F08_PREP"), crate::tests::test_world(), rc_testdata::version_data())
    else {
        return;
    };
    let live = std::env::var("RUSTCRAFT_F08_SCENE").is_ok_and(|s| s == "live");
    let dir = super::super::tests::prep(&world, "f08prep", if live { 0 } else { 1 }, "normal", &[]);
    {
        let mut p = Probe::open(&dir, std::sync::Arc::new(data));
        p.run(
            "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -50 231 minecraft:air\nfill 200 -59 200 231 -53 231 minecraft:water\nplayer 202.5 -58 216.5 -90 5\n",
        );
        p.s.view = 8;
        set_day_time(&mut p, 6000);
        let g = spawn(&mut p, Kind::Guardian, [206.5, -57.5, 214.5]);
        let e = spawn(&mut p, Kind::ElderGuardian, [211.0, -58.5, 218.5]);
        let d = spawn(&mut p, Kind::Drowned, [206.0, -59.0, 218.5]);
        let d2 = spawn(&mut p, Kind::Drowned, [208.5, -59.0, 212.0]);
        for id in [d, d2] {
            if let Some(i) = p.s.mob_index(id)
                && let Some(m) = p.s.mobs.list[i].as_deref_mut()
            {
                m.equipment[0] =
                    Some(crate::player::Stack { id: "minecraft:trident".into(), count: 1, components: None });
            }
        }
        let _ = (g, e);
        for m in p.s.mobs.list.iter_mut().flatten() {
            m.yaw = 90.0;
            m.body_yaw = 90.0;
            m.head_yaw = 90.0;
            m.no_ai = !live;
            m.persistent = true;
        }
        step(&mut p, 2);
        p.run("save\n");
    }
    super::super::tests::move_player(&dir, [202.5, -58.0, 216.5]);
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) == Some("dat") {
            let mut pl = crate::nbtio::read_dat(&path).unwrap();
            crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(-90.0), Nbt::Float(5.0)]));
            std::fs::write(&path, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
        }
    }
    let dest = std::path::PathBuf::from(dest);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&dir, &dest).unwrap();
    eprintln!("F08 world prepared: {}", dest.display());
}

fn enchanted_trident(name: &str, level: i32) -> crate::player::Stack {
    let ench = Nbt::Compound(vec![(name.to_owned(), Nbt::Int(level))]);
    crate::player::Stack {
        id: "minecraft:trident".into(),
        count: 1,
        components: Some(Nbt::Compound(vec![("minecraft:enchantments".into(), ench)])),
    }
}

/// Loyalty trident: after impact it returns to the drowned and disappears near it.
#[test]
fn loyalty_trident_returns_to_owner() {
    let Some((mut p, dir)) = probe("f08loyal") else { return };
    p.run("player 202.5 -59 202.5 0 0\n");
    let d = spawn(&mut p, Kind::Drowned, [208.5, -59.0, 216.5]);
    if let Some(i) = p.s.mob_index(d)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.no_ai = true;
        m.persistent = true;
    }
    let owner = mob(&p, d).unwrap().clone();
    p.s.spawn_trident(&owner, [208.5, -58.0, 216.5], [1.0, 0.0, 0.0], 0.0, enchanted_trident("minecraft:loyalty", 2));
    assert_eq!(p.s.proj.list[0].loyalty, 2);
    // No target: embedded in the stone floor, then recalled after 5 ticks on the ground.
    let mut gone_at = None;
    for t in 0..400 {
        step(&mut p, 1);
        if p.s.proj.list.is_empty() {
            gone_at = Some(t);
            break;
        }
    }
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(gone_at.is_some_and(|t| t < 300), "the Loyalty trident did not return: {gone_at:?}");
}

/// Trident without Loyalty: stays embedded (removed after 1,200 ticks only).
#[test]
fn plain_trident_stays_planted() {
    let Some((mut p, dir)) = probe("f08plain") else { return };
    p.run("player 202.5 -59 202.5 0 0\n");
    let d = spawn(&mut p, Kind::Drowned, [208.5, -59.0, 216.5]);
    if let Some(i) = p.s.mob_index(d)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.no_ai = true;
        m.persistent = true;
    }
    let owner = mob(&p, d).unwrap().clone();
    p.s.spawn_trident(&owner, [208.5, -58.0, 216.5], [1.0, 0.0, 0.0], 0.0, enchanted_trident("minecraft:impaling", 3));
    for _ in 0..200 {
        step(&mut p, 1);
    }
    let t = p.s.proj.list.first().map(|t| (t.in_ground, t.vel));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(t.is_some_and(|(g, v)| g && v == [0.0; 3]), "trident not embedded or speed not zero: {t:?}");
}

/// Zombie and drowned crush nearby turtle eggs (priority 4, 60 ticks in place).
#[test]
fn zombie_and_drowned_crush_turtle_eggs() {
    for (k, tag) in [(Kind::Zombie, "f08egg_z"), (Kind::Drowned, "f08egg_d")] {
        let Some((mut p, dir)) = probe(tag) else { return };
        set_day_time(&mut p, 18000);
        p.run("player 202.5 -59 202.5 0 0\nsetblock 214 -59 216 minecraft:turtle_egg\n");
        assert_eq!(state_string(&p.s, p.s.get([214, -59, 216])).split('[').next(), Some("minecraft:turtle_egg"));
        // Creative: no target, only the egg goal can act.
        p.s.game_mode = 1;
        let z = spawn(&mut p, k, [210.5, -59.0, 216.5]);
        if let Some(i) = p.s.mob_index(z)
            && let Some(m) = p.s.mobs.list[i].as_deref_mut()
        {
            m.persistent = true;
        }
        let mut broken = None;
        for t in 0..600 {
            step(&mut p, 1);
            if state_string(&p.s, p.s.get([214, -59, 216])) == "minecraft:air" {
                broken = Some(t);
                break;
            }
        }
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(broken.is_some(), "{k:?} did not crush the egg");
    }
}

/// Guardians and the like: events only, no step or swim sound.
#[test]
fn movement_emission_sounds() {
    use super::splash::emits_sounds;
    assert!(!emits_sounds(Kind::Guardian) && !emits_sounds(Kind::ElderGuardian));
    assert!(emits_sounds(Kind::Drowned) && emits_sounds(Kind::Zombie));
}
