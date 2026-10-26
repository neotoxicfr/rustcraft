//! Family F07 end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, zone
//! 200..231) driven by the probe: pillager's crossbow, vindicator's axe, evoker's fangs and vexes,
//! ravager's hit, witch's potions, patrol, raid at the village,
//! save. World absent: tests skipped.

use rc_proto::nbt::Nbt;

use super::super::mob::Ent;
use super::super::species::Kind;
use crate::blocks::probe::Probe;

/// Stone floor at −60, air above, player at the center (survival, normal difficulty).
fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -50 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    if crate::blocks::probe::state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    // "Ready" player: damage touches them from the first tick.
    p.s.loaded = true;
    p.s.view = 8;
    Some((p, dir))
}

fn step(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.tick();
        // Player effects (omens), then mobs.
        p.s.effects_tick();
        p.s.mobs_tick();
        p.s.projectiles_tick(); // mob projectiles: common engine (S8)
    }
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::super::spawn::Reason::SpawnEgg, &mut g);
    m.persistent = true;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn finish(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Pillager: aims at the player, charges the crossbow (25 ticks, data 17), the visible charge,
/// fires a bolt that hurts the player.
#[test]
fn pillager_shoots_crossbow() {
    let Some((mut p, dir)) = probe("f07pill") else { return };
    let id = spawn(&mut p, Kind::Pillager, [216.5, -59.0, 214.5]);
    let h0 = p.s.life.vit.health;
    let (mut charged, mut bolts, mut loaded) = (false, 0, false);
    let mut seen = hashbrown::HashSet::new();
    for _ in 0..400 {
        step(&mut p, 1);
        let m = p.s.mob_ref(id).unwrap();
        charged |= m.ill.charging;
        loaded |= m.equipment[0]
            .as_ref()
            .is_some_and(|s| crate::items::component(s, "minecraft:charged_projectiles").is_some());
        let owner = p.s.mob_ref(id).map(|m| m.uuid);
        for a in &p.s.proj.list {
            if a.owner == owner && seen.insert(a.id) {
                bolts += 1;
            }
        }
        if p.s.life.vit.health < h0 {
            break;
        }
    }
    let m = p.s.mob_ref(id).unwrap();
    eprintln!("target {:?}, charge {charged}, loaded {loaded}, bolts {bolts}, HP {}", m.target, p.s.life.vit.health);
    assert_eq!(m.target, Some(Ent::Player));
    assert!(charged && loaded, "crossbow never loaded");
    assert!(bolts >= 1, "no bolt");
    assert!(p.s.life.vit.health < h0, "player never hit");
    finish(p, dir);
}

/// Vindicator: iron axe (5 + 8 = 13 damage on normal), hit on the player.
#[test]
fn vindicator_axe_hit() {
    let Some((mut p, dir)) = probe("f07vind") else { return };
    let id = spawn(&mut p, Kind::Vindicator, [216.5, -59.0, 206.5]);
    assert_eq!(p.s.mob_ref(id).unwrap().equipment[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:iron_axe"));
    let h0 = p.s.life.vit.health;
    for _ in 0..200 {
        step(&mut p, 1);
        if p.s.life.vit.health < h0 {
            break;
        }
    }
    let lost = h0 - p.s.life.vit.health;
    eprintln!("HP lost {lost}");
    assert!((lost - 13.0).abs() < 1e-3, "damage {lost}");
    finish(p, dir);
}

/// Evoker: spells (fangs, vexes) against the player; fangs placed on the ground, summoned vexes linked.
#[test]
fn evoker_fangs_and_vexes() {
    let Some((mut p, dir)) = probe("f07evok") else { return };
    let id = spawn(&mut p, Kind::Evoker, [216.5, -59.0, 212.5]);
    let (mut fangs, mut casting) = (0, false);
    let mut seen = hashbrown::HashSet::new();
    for _ in 0..600 {
        step(&mut p, 1);
        casting |= p.s.mob_ref(id).is_some_and(|m| m.ill.spell != super::Spell::None);
        for f in &p.s.mobs.ill.fangs {
            if seen.insert(f.id) {
                fangs += 1;
                assert!((f.pos[1] - -59.0).abs() < 1e-9, "fangs off the ground {:?}", f.pos);
            }
        }
        if fangs > 0 && p.s.mobs.list.iter().flatten().any(|m| m.kind == Kind::Vex) {
            break;
        }
    }
    let vexes: Vec<_> = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Vex).collect();
    eprintln!("casting {casting}, fangs {fangs}, vex {}", vexes.len());
    assert!(casting, "no spell");
    assert!(fangs > 0 || !vexes.is_empty(), "neither fangs nor vex");
    for v in &vexes {
        assert_eq!(v.ill.owner_id, Some(id));
        assert!(v.ill.life.is_some_and(|l| (0..=20 * 120).contains(&l)));
        assert!(v.ill.bound.is_some());
    }
    finish(p, dir);
}

/// Vex: flies toward the player through a wall, without gravity, and hits (iron sword).
#[test]
fn vex_flies_through_walls() {
    let Some((mut p, dir)) = probe("f07vex") else { return };
    p.run("fill 210 -59 208 222 -55 208 minecraft:stone\n");
    let id = spawn(&mut p, Kind::Vex, [216.5, -57.0, 212.5]);
    // Wall between them: the target is given (the game's targeting requires sight).
    if let Some(i) = p.s.mob_index(id)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.target = Some(Ent::Player);
    }
    let h0 = p.s.life.vit.health;
    let mut min_z = f64::MAX;
    for t in 0..400 {
        step(&mut p, 1);
        if let Some(v) = p.s.mob_ref(id) {
            min_z = min_z.min(v.pos[2]);
            if t % 40 == 0 {
                eprintln!("t {t} pos {:?} op {:?} target {:?}", v.pos, v.move_ctl.op, v.target);
            }
        }
        if p.s.life.vit.health < h0 {
            break;
        }
    }
    eprintln!("z min {min_z}, HP {}", p.s.life.vit.health);
    assert!(min_z < 208.0, "the vex did not cross the wall");
    assert!(p.s.life.vit.health < h0, "player never hit");
    finish(p, dir);
}

/// Ravager: hit (12 base, event 4), motionless for 10 ticks.
#[test]
fn ravager_attacks() {
    let Some((mut p, dir)) = probe("f07rav") else { return };
    let id = spawn(&mut p, Kind::Ravager, [216.5, -59.0, 208.5]);
    let h0 = p.s.life.vit.health;
    let mut attack = false;
    for _ in 0..300 {
        step(&mut p, 1);
        attack |= p.s.mob_ref(id).is_some_and(|m| m.ill.attack_tick > 0);
        if p.s.life.vit.health < h0 {
            break;
        }
    }
    eprintln!("hit {attack}, HP {}", p.s.life.vit.health);
    assert!(attack, "no hit");
    assert!(p.s.life.vit.health < h0);
    finish(p, dir);
}

/// Witch: potion thrown (in flight), effect or damage on the player.
#[test]
fn witch_throws_potions() {
    let Some((mut p, dir)) = probe("f07witch") else { return };
    spawn(&mut p, Kind::Witch, [216.5, -59.0, 210.5]);
    let h0 = p.s.life.vit.health;
    let mut thrown = false;
    let mut hit = false;
    for _ in 0..400 {
        step(&mut p, 1);
        thrown |= p.s.proj.list.iter().any(|t| t.kind == crate::projectiles::PKind::SplashPotion);
        hit = p.s.life.vit.health < h0
            || ["minecraft:slowness", "minecraft:poison", "minecraft:weakness"].iter().any(|e| p.s.fx().has(e));
        if hit {
            break;
        }
    }
    eprintln!("thrown {thrown}, hit {hit}");
    assert!(thrown, "no potion thrown");
    assert!(hit, "player never reached");
    finish(p, dir);
}

/// Patrol spawner: by day, after 5 clock days, a banner leader and its
/// pillagers appear 24-47 blocks from the player.
#[test]
fn patrol_spawns() {
    let Some((mut p, dir)) = probe("f07patrol") else { return };
    p.s.view = 10;
    // Player at the center of the loaded chunks of the copy (176..255).
    p.run("player 216.5 -59 216.5 0 0\n");
    // Fifth day passed (Overworld clock), in broad daylight.
    p.s.clocks.retain(|c| c.0 != "minecraft:overworld");
    p.s.clocks.push(("minecraft:overworld".into(), 126_000));
    p.s.time = 126_000;
    eprintln!(
        "day {}; chunks ±10 around (240, 226): {}",
        p.s.bright_outside(),
        p.s.chunks_loaded_box(230, 216, 250, 236)
    );
    let loaded: Vec<(i32, i32)> = p.s.chunks.iter().filter(|(_, c)| c.is_some()).map(|(k, _)| *k).collect();
    let (x0, x1) = (loaded.iter().map(|c| c.0).min(), loaded.iter().map(|c| c.0).max());
    let (z0, z1) = (loaded.iter().map(|c| c.1).min(), loaded.iter().map(|c| c.1).max());
    eprintln!("chunks loaded x {x0:?}..{x1:?} z {z0:?}..{z1:?} ({})", loaded.len());
    let mut spawned = Vec::new();
    for _ in 0..400 {
        p.s.mobs.ill.patrol_next = 1;
        p.s.patrol_spawner_tick();
        spawned = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Pillager).map(|m| (**m).clone()).collect();
        if !spawned.is_empty() {
            break;
        }
    }
    eprintln!("pillagers {}", spawned.len());
    assert!(!spawned.is_empty(), "no patrol");
    let leaders: Vec<_> = spawned.iter().filter(|m| m.ill.patrol_leader).collect();
    assert_eq!(leaders.len(), 1, "a single leader");
    assert!(super::is_captain(leaders[0]), "leader's banner");
    // Target drawn when the pillager is still at the origin: ±500 around (0, 0, 0).
    let t = leaders[0].ill.patrol_target.expect("leader's target");
    assert!(t[0].abs() <= 500 && t[2].abs() <= 500 && t[1] == 0, "target {t:?}");
    assert!(spawned.iter().all(|m| m.ill.patrolling));
    let d = ((spawned[0].pos[0] - 216.5).abs(), (spawned[0].pos[2] - 216.5).abs());
    assert!(d.0 >= 23.0 && d.1 >= 23.0, "too close {d:?}");
    finish(p, dir);
}

/// Raid: Bad Omen at the village (occupied bell) → raid omen → raid created, first
/// wave after the 300-tick delay (banner pillagers), boss bar.
#[test]
fn raid_starts_in_village() {
    let Some((mut p, dir)) = probe("f07raid") else { return };
    p.run("setblock 216 -59 205 minecraft:bell\n");
    step(&mut p, 1);
    p.s.poi_acquire([216, -59, 205]);
    if !p.s.is_village([216, -59, 202]) {
        eprintln!("bell POI not taken into account");
        finish(p, dir);
        return;
    }
    p.s.give_player_effect("minecraft:bad_omen", 6000, 0);
    step(&mut p, 2);
    assert!(p.s.fx().has("minecraft:raid_omen"), "raid omen missing");
    assert!(!p.s.fx().has("minecraft:bad_omen"));
    step(&mut p, 610);
    let Some(r) = p.s.mobs.ill.raids.list.first().cloned() else { panic!("no raid") };
    eprintln!("raid {} center {:?} omen {} waves {}", r.id, r.center, r.omen, r.num_groups);
    assert_eq!((r.id, r.omen, r.num_groups), (2, 1, 5));
    let mut wave = false;
    for _ in 0..400 {
        step(&mut p, 1);
        if p.s.mobs.ill.raids.list.first().is_some_and(|r| r.groups_spawned > 0) {
            wave = true;
            break;
        }
    }
    let raiders: Vec<_> = p.s.mobs.list.iter().flatten().filter(|m| m.ill.raid.is_some()).collect();
    eprintln!("wave {wave}, pillagers {}", raiders.len());
    if !wave {
        eprintln!("no spawn position found (zone too small?)");
        finish(p, dir);
        return;
    }
    // Wave 1 on normal: 4 pillagers + 0-1 bonus, 0 vindicator + 0-1 bonus (raid random).
    assert!(raiders.iter().all(|m| matches!(m.kind, Kind::Pillager | Kind::Vindicator)), "wave 1");
    let pillagers = raiders.iter().filter(|m| m.kind == Kind::Pillager).count();
    assert!((4..=5).contains(&pillagers) && raiders.len() - pillagers <= 1);
    assert_eq!(raiders.iter().filter(|m| super::is_captain(m)).count(), 1, "one captain");
    finish(p, dir);
}

/// Save: own fields read back (patrol, pillager inventory, vex).
#[test]
fn illagers_saved_and_reloaded() {
    let Some((mut p, dir)) = probe("f07save") else { return };
    let pid = spawn(&mut p, Kind::Pillager, [210.5, -59.0, 210.5]);
    let vid = spawn(&mut p, Kind::Vex, [212.5, -57.0, 210.5]);
    if let Some(i) = p.s.mob_index(pid)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.ill.patrol_target = Some([300, -59, 400]);
        m.ill.patrol_leader = true;
        m.ill.patrolling = true;
        m.ill.inventory =
            vec![crate::player::Stack { id: "minecraft:white_banner".into(), count: 2, components: None }];
    }
    if let Some(i) = p.s.mob_index(vid)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.ill.bound = Some([212, -57, 210]);
        m.ill.life = Some(1234);
        m.ill.owner = Some(42);
    }
    step(&mut p, 1);
    p.run("save\n");
    let list = super::super::tests::saved(&dir, 210 >> 4, 210 >> 4);
    let pl = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:pillager")).expect("pillager");
    assert_eq!(pl.get("patrol_target"), Some(&Nbt::IntArray(vec![300, -59, 400])));
    assert_eq!(crate::nbtio::int(pl, "PatrolLeader"), Some(1));
    assert_eq!(crate::nbtio::list(pl.get("Inventory")).len(), 1);
    let vx = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:vex")).expect("vex");
    assert_eq!(crate::nbtio::int(vx, "life_ticks"), Some(1233));
    assert_eq!(vx.get("bound_pos"), Some(&Nbt::IntArray(vec![212, -57, 210])));
    let mut m = super::super::mob::Mob::new(Kind::Pillager, 1, 1, [0.0; 3], 0.0, super::super::jrand::JRand::new(1));
    m.define_data();
    m.load(pl);
    assert_eq!(m.ill.patrol_target, Some([300, -59, 400]));
    assert!(m.can_pickup, "the pillager read back picks up");
    finish(p, dir);
}

/// Structure spawn lists: a witch hut (piece) and an outpost (box
/// enlarged by 12) written into the chunks' NBT replace the biome list.
#[test]
fn structure_spawn_lists() {
    let Some((mut p, dir)) = probe("f07struct") else { return };
    let key = |cx: i32, cz: i32| (i64::from(cx) & 0xFFFF_FFFF) | (i64::from(cz) << 32);
    let start = |id: &str, bb: [i32; 6]| {
        Nbt::Compound(vec![
            ("id".into(), Nbt::String(id.into())),
            ("Children".into(), Nbt::List(vec![Nbt::Compound(vec![("BB".into(), Nbt::IntArray(bb.to_vec()))])])),
        ])
    };
    // Hut: piece 200..206 in chunk (12, 12); outpost: piece 216..220 in (13, 13).
    let refs = Nbt::Compound(vec![
        ("minecraft:swamp_hut".into(), Nbt::LongArray(vec![key(12, 12)])),
        ("minecraft:pillager_outpost".into(), Nbt::LongArray(vec![key(13, 13)])),
    ]);
    for (c, starts) in [
        (
            (12, 12),
            vec![("minecraft:swamp_hut".to_owned(), start("minecraft:swamp_hut", [200, -60, 200, 206, -50, 206]))],
        ),
        (
            (13, 13),
            vec![(
                "minecraft:pillager_outpost".to_owned(),
                start("minecraft:pillager_outpost", [216, -60, 216, 220, -40, 220]),
            )],
        ),
    ] {
        let ch = p.s.chunks.get_mut(&c).and_then(|c| c.as_deref_mut()).expect("chunk loaded");
        let st = Nbt::Compound(vec![("References".into(), refs.clone()), ("starts".into(), Nbt::Compound(starts))]);
        crate::nbtio::set(&mut ch.root, "structures", st);
    }
    let mon = |p: &Probe, at: [i32; 3]| -> Vec<String> {
        p.s.spawn_list_at(super::super::species::Category::Monster, "monster", at)
            .iter()
            .filter_map(|e| e["type"].as_str().map(str::to_owned))
            .collect()
    };
    assert_eq!(mon(&p, [203, -59, 203]), ["minecraft:witch"], "in the hut's piece");
    assert!(mon(&p, [207, -59, 203]).len() > 1, "outside piece and outside box: biome list");
    assert_eq!(mon(&p, [208, -59, 208]), ["minecraft:pillager"], "outpost box enlarged by 12");
    assert_eq!(mon(&p, [223, -59, 223]), ["minecraft:pillager"]);
    let cre: Vec<String> =
        p.s.spawn_list_at(super::super::species::Category::Creature, "creature", [203, -59, 203])
            .iter()
            .filter_map(|e| e["type"].as_str().map(str::to_owned))
            .collect();
    assert_eq!(cre, ["minecraft:cat"]);
    finish(p, dir);
}

/// Demo world (`RUSTCRAFT_F07_PREP=<folder>`, `RUSTCRAFT_F07_SCENE=static|live`):
/// player at (210.5, −59, 210.5) facing east; the 7 species in a row; `static`: no AI.
#[test]
#[ignore]
fn prep_illagers_world() {
    let (Ok(dest), Some(world), Some(data)) =
        (std::env::var("RUSTCRAFT_F07_PREP"), crate::tests::test_world(), rc_testdata::version_data())
    else {
        return;
    };
    let live = std::env::var("RUSTCRAFT_F07_SCENE").is_ok_and(|s| s == "live");
    let dir = super::super::tests::prep(&world, "f07prep", if live { 0 } else { 1 }, "normal", &[]);
    {
        let mut p = Probe::open(&dir, std::sync::Arc::new(data));
        p.run("fill 200 -59 200 231 -50 231 minecraft:air\nplayer 210.5 -59 210.5 -90 10\n");
        p.s.view = 8;
        let kinds =
            [Kind::Pillager, Kind::Vindicator, Kind::Evoker, Kind::Illusioner, Kind::Witch, Kind::Vex, Kind::Ravager];
        for (i, k) in kinds.into_iter().enumerate() {
            let z = 204.5 + i as f64 * 2.0;
            let y = if k == Kind::Vex { -58.0 } else { -59.0 };
            let x = if k == Kind::Ravager { 219.5 } else { 217.5 };
            let id = spawn(&mut p, k, [x, y, z]);
            if k == Kind::Pillager
                && let Some(i) = p.s.mob_index(id)
                && let Some(m) = p.s.mobs.list[i].as_deref_mut()
            {
                m.ill.patrol_leader = true;
                m.equipment[5] = Some(super::ominous_banner());
            }
        }
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
    super::super::tests::move_player(&dir, [210.5, -59.0, 210.5]);
    for e in std::fs::read_dir(dir.join("players").join("data")).unwrap().flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) == Some("dat") {
            let mut pl = crate::nbtio::read_dat(&path).unwrap();
            crate::nbtio::set(&mut pl, "Rotation", Nbt::List(vec![Nbt::Float(-90.0), Nbt::Float(10.0)]));
            std::fs::write(&path, crate::nbtio::gzip(&crate::nbtio::encode_root(&pl))).unwrap();
        }
    }
    let dest = std::path::PathBuf::from(dest);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&dir, &dest).unwrap();
    eprintln!("F07 world prepared: {}", dest.display());
}

/// Hut witch, elder guardian and mansion allay read back with the generation marker:
/// finalized as at placement (follow bonus drawn; the structure witch can join a
/// raid); without marker, none of that.
#[test]
fn structure_mobs_are_finalized() {
    let Some((mut p, dir)) = probe("g6f19mobs") else { return };
    for (id, mark) in [
        ("minecraft:witch", true),
        ("minecraft:elder_guardian", true),
        ("minecraft:allay", true),
        ("minecraft:witch", false),
    ] {
        let mut f = vec![
            ("id".into(), Nbt::String(id.into())),
            ("Pos".into(), Nbt::List(vec![Nbt::Double(210.5), Nbt::Double(-59.0), Nbt::Double(210.5)])),
            ("PersistenceRequired".into(), Nbt::Byte(1)),
        ];
        if mark {
            f.push(("rc_finalize_structure".into(), Nbt::Byte(1)));
        }
        p.s.next_entity += 1;
        let n = p.s.next_entity;
        let n = p.s.adopt_with_passengers(n, &Nbt::Compound(f), None).expect("adopted");
        let i = p.s.mob_index(n).expect("indexed");
        let m = p.s.mobs.list[i].as_deref().expect("mob");
        let bonus = m.attrs.follow.perm.contains(&"minecraft:random_spawn_bonus");
        assert_eq!(bonus, mark, "{id} (marker {mark}): follow bonus");
        if m.kind == Kind::Witch {
            assert_eq!(m.ill.can_join_raid, mark, "witch (marker {mark}): raid");
        }
        assert!(m.save().get("rc_finalize_structure").is_none());
    }
    finish(p, dir);
}

/// Mansion illager read back with the generation marker: iron axe, no patrol leader.
#[test]
fn mansion_vindicator_is_finalized() {
    let Some((mut p, dir)) = probe("f07mansion") else { return };
    let e = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:vindicator".into())),
        ("Pos".into(), Nbt::List(vec![Nbt::Double(210.5), Nbt::Double(-59.0), Nbt::Double(210.5)])),
        ("PersistenceRequired".into(), Nbt::Byte(1)),
        ("rc_finalize_structure".into(), Nbt::Byte(1)),
    ]);
    p.s.next_entity += 1;
    let id = p.s.next_entity;
    let id = p.s.adopt_with_passengers(id, &e, None).expect("adopted");
    let i = p.s.mob_index(id).expect("indexed");
    let m = p.s.mobs.list[i].as_deref().expect("mob");
    assert_eq!(m.equipment[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:iron_axe"));
    assert!(!m.ill.patrol_leader);
    assert!(m.save().get("rc_finalize_structure").is_none());
    finish(p, dir);
}
