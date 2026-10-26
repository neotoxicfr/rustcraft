//! Golems end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, generated zone
//! 200..231) driven by the probe: construction from blocks (standing, lying), copper golem and
//! chest, snow trail and snowballs, iron golem against zombie, chest sorting.

use rc_proto::nbt::Nbt;

use super::super::species::Kind;
use crate::blocks::probe::{Probe, state_string};

/// Stone floor at −60, air above, player at the center (survival, normal difficulty).
fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -50 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    if name_at(&p, [216, -60, 216]) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    // The whole simulated zone (mobs within `view − 2` chunks of the player).
    p.s.view = 8;
    Some((p, dir))
}

/// Full ticks: blocks then mobs.
fn step(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.tick();
        p.s.mobs_tick();
        p.s.projectiles_tick(); // mob projectiles: common engine (S8)
    }
}

fn golems(p: &Probe, k: Kind) -> Vec<([f64; 3], i32)> {
    p.s.mobs.list.iter().flatten().filter(|m| m.kind == k && !m.removed).map(|m| (m.pos, m.id)).collect()
}

fn name_at(p: &Probe, at: [i32; 3]) -> String {
    state_string(&p.s, p.s.get(at))
}

/// Standing iron golem (T in z) and lying snow golem (head to the east): pattern erased, golem
/// placed on the bottom cell (+0.05), iron "built by a player".
#[test]
fn golems_built_from_blocks() {
    let Some((mut p, dir)) = probe("f06build") else { return };
    p.run(
        "setblock 205 -59 210 minecraft:iron_block\nsetblock 205 -58 210 minecraft:iron_block\nsetblock 205 -58 209 minecraft:iron_block\nsetblock 205 -58 211 minecraft:iron_block\nsetblock 205 -57 210 minecraft:carved_pumpkin\n",
    );
    let iron = golems(&p, Kind::IronGolem);
    eprintln!("iron: {iron:?}");
    assert_eq!(iron.len(), 1, "iron golem not built");
    assert_eq!(iron[0].0, [205.5, -59.0 + 0.05, 210.5]);
    let g = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::IronGolem).unwrap();
    assert!(g.golem.player_created);
    for q in [[205, -59, 210], [205, -58, 210], [205, -58, 209], [205, -58, 211], [205, -57, 210]] {
        assert_eq!(name_at(&p, q), "minecraft:air", "{q:?}");
    }
    // Lying: pumpkin to the east of the two snow blocks.
    p.run(
        "setblock 210 -59 220 minecraft:snow_block\nsetblock 211 -59 220 minecraft:snow_block\nsetblock 212 -59 220 minecraft:jack_o_lantern\n",
    );
    let snow = golems(&p, Kind::SnowGolem);
    eprintln!("snow: {snow:?}");
    assert_eq!(snow.len(), 1, "lying snow golem not built");
    assert_eq!(snow[0].0, [210.5, -59.0 + 0.05, 220.5]);
    // Incomplete pattern: no golem.
    p.run("setblock 220 -59 220 minecraft:iron_block\nsetblock 220 -58 220 minecraft:carved_pumpkin\n");
    assert_eq!(golems(&p, Kind::IronGolem).len(), 1);
    assert_eq!(name_at(&p, [220, -58, 220]), "minecraft:carved_pumpkin[facing=north]");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Copper golem: exposed copper block + pumpkin turned east → exposed golem on the pumpkin's
/// cell, exposed copper chest oriented east in place of the block.
#[test]
fn copper_golem_built_with_chest() {
    let Some((mut p, dir)) = probe("f06copper") else { return };
    p.run(
        "setblock 220 -59 212 minecraft:exposed_copper\nsetblock 220 -58 212 minecraft:carved_pumpkin[facing=east]\n",
    );
    let cu = golems(&p, Kind::CopperGolem);
    eprintln!("copper: {cu:?}, block {}", name_at(&p, [220, -59, 212]));
    assert_eq!(cu.len(), 1, "copper golem not built");
    assert_eq!(cu[0].0, [220.5, -58.0 + 0.05, 212.5]);
    let g = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::CopperGolem).unwrap();
    assert_eq!(g.golem.weather, 1);
    assert!(name_at(&p, [220, -59, 212]).starts_with("minecraft:exposed_copper_chest[facing=east,type=single"));
    assert!(p.s.be_get([220, -59, 212]).is_some(), "chest block entity");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let m = p.s.new_mob(k, pos, 0.0);
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// Snow golem facing a zombie (husk): it targets it, throws snowballs (the zombie, hit, takes it
/// as a target) and leaves a snow trail.
///
/// An ordinary zombie burns in the sun: 1 damage every 20 ticks, then 20 ticks of
/// invulnerability during which a snowball (0 damage) is refused for the first half; the golem
/// also shoots every 20 ticks, in fixed phase with the fire, and with this phase no snowball
/// ever counted (14 throws without a hit, one time in four). The husk
/// does not burn. Player in creative: in survival, the zombie sometimes went for them.
#[test]
fn snow_golem_shoots_and_trails() {
    let Some((mut p, dir)) = probe("f06snow") else { return };
    p.s.game_mode = 1;
    let golem = spawn(&mut p, Kind::SnowGolem, [210.5, -59.0, 216.5]);
    let zombie = spawn(&mut p, Kind::Husk, [217.5, -59.0, 216.5]);
    let (mut thrown, mut targeted, mut hurt_by) = (0, false, false);
    let mut seen = hashbrown::HashSet::new();
    for _ in 0..300 {
        step(&mut p, 1);
        for b in p.s.proj.list.iter().filter(|b| b.kind == crate::projectiles::PKind::Snowball) {
            seen.insert(b.id);
        }
        thrown = seen.len();
        let g = p.s.mob_ref(golem);
        targeted |= g.is_some_and(|g| g.target == Some(super::super::mob::Ent::Mob(zombie)));
        hurt_by |=
            p.s.mob_ref(zombie)
                .is_some_and(|z| z.last_hurt_by_mob.is_some_and(|(e, _)| e == super::super::mob::Ent::Mob(golem)));
        if thrown >= 2 && hurt_by {
            break;
        }
    }
    let mut snow = 0;
    for x in 200..232 {
        for z in 200..232 {
            if name_at(&p, [x, -59, z]).starts_with("minecraft:snow[") {
                snow += 1;
            }
        }
    }
    eprintln!("targeted {targeted}, balls {thrown}, zombie hit {hurt_by}, snow {snow}");
    assert!(targeted, "the snow golem does not target the zombie");
    assert!(thrown >= 1, "no snowball");
    assert!(hurt_by, "zombie never hit");
    assert!(snow >= 1, "no snow trail");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Iron golem against zombie: hits (event 4, sound), zombie killed.
#[test]
fn iron_golem_kills_zombie() {
    let Some((mut p, dir)) = probe("f06iron") else { return };
    let golem = spawn(&mut p, Kind::IronGolem, [210.5, -59.0, 216.5]);
    let zombie = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 216.5]);
    let mut ticks = 0;
    let mut anim = false;
    while ticks < 600 {
        step(&mut p, 1);
        ticks += 1;
        anim |= p.s.mob_ref(golem).is_some_and(|g| g.golem.attack_anim > 0);
        if p.s.mob_ref(zombie).is_none_or(|z| z.dead || z.health <= 0.0) {
            break;
        }
    }
    let alive = p.s.mob_ref(zombie).is_some_and(|z| !z.dead && z.health > 0.0);
    eprintln!("zombie dead {} after {ticks} ticks, hit seen {anim}", !alive);
    assert!(anim, "the golem did not hit");
    assert!(!alive, "zombie still alive");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sorting: 20 cobblestones in a copper chest, copper golem between it and a chest; at the end
/// of the transport, 16 are in the chest (a stack of 16 at most per trip).
#[test]
fn copper_golem_sorts_items() {
    let Some((mut p, dir)) = probe("f06sort") else { return };
    p.run(
        "setblock 216 -59 216 minecraft:copper_chest[facing=south]\ndata merge block 216 -59 216 {Items:[{Slot:0b,id:\"minecraft:cobblestone\",count:20}]}\nsetblock 216 -59 222 minecraft:chest[facing=north]\n",
    );
    let golem = spawn(&mut p, Kind::CopperGolem, [216.5, -59.0, 219.5]);
    let dest = [216, -59, 222];
    let mut ticks = 0;
    let mut held = false;
    while ticks < 1500 {
        step(&mut p, 1);
        ticks += 1;
        held |= p.s.mob_ref(golem).is_some_and(|g| g.equipment[0].is_some());
        if p.s.be_item_counts(dest).iter().map(|e| e.1).sum::<i32>() > 0 {
            break;
        }
    }
    let moved: i32 = p.s.be_item_counts(dest).iter().map(|e| e.1).sum();
    let left: i32 = p.s.be_item_counts([216, -59, 216]).iter().map(|e| e.1).sum();
    eprintln!("after {ticks} ticks: held {held}, chest {moved}, copper chest {left}");
    assert!(held, "the golem took nothing");
    assert_eq!(moved, 16, "items not deposited");
    assert_eq!(left, 4);
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Demo world (`RUSTCRAFT_F06_PREP=<folder>`, `RUSTCRAFT_F06_SCENE=static|live`): player at
/// (210.5, −59, 210.5) facing east; iron and snow golems, four copper golems (intact to
/// oxidized), an iron golem built from blocks; `static`: no AI.
#[test]
#[ignore]
fn prep_golems_world() {
    let (Ok(dest), Some(world), Some(data)) =
        (std::env::var("RUSTCRAFT_F06_PREP"), crate::tests::test_world(), rc_testdata::version_data())
    else {
        return;
    };
    let live = std::env::var("RUSTCRAFT_F06_SCENE").is_ok_and(|s| s == "live");
    let dir = super::super::tests::prep(
        &world,
        "f06prep",
        if live { 0 } else { 1 },
        "normal",
        &[("carved_pumpkin", 4), ("iron_ingot", 8), ("shears", 1), ("honeycomb", 4), ("iron_axe", 1)],
    );
    {
        let mut p = Probe::open(&dir, std::sync::Arc::new(data));
        p.run("fill 200 -59 200 231 -50 231 minecraft:air\nplayer 210.5 -59 210.5 -90 10\n");
        p.s.view = 8;
        // Iron golem built from blocks (T in z).
        p.run(
            "setblock 219 -59 210 minecraft:iron_block\nsetblock 219 -58 210 minecraft:iron_block\nsetblock 219 -58 209 minecraft:iron_block\nsetblock 219 -58 211 minecraft:iron_block\nsetblock 219 -57 210 minecraft:carved_pumpkin[facing=west]\n",
        );
        spawn(&mut p, Kind::IronGolem, [216.5, -59.0, 206.5]);
        spawn(&mut p, Kind::SnowGolem, [215.5, -59.0, 213.5]);
        for w in 0..4 {
            let id = spawn(&mut p, Kind::CopperGolem, [213.5, -59.0, 207.5 + f64::from(w) * 1.6]);
            if let Some(i) = p.s.mob_index(id)
                && let Some(m) = p.s.mobs.list[i].as_deref_mut()
            {
                m.golem.weather = w;
                m.golem.next_weather = -2;
            }
        }
        if live {
            // Copper chest filled and chest: sorting visible.
            p.run(
                "setblock 212 -59 216 minecraft:copper_chest[facing=west]\ndata merge block 212 -59 216 {Items:[{Slot:0b,id:\"minecraft:cobblestone\",count:40}]}\nsetblock 216 -59 217 minecraft:chest[facing=west]\n",
            );
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
    eprintln!("F06 world prepared: {}", dest.display());
}

/// Save: golems read back with their own fields.
#[test]
fn golems_saved_and_reloaded() {
    let Some((mut p, dir)) = probe("f06save") else { return };
    let id = spawn(&mut p, Kind::CopperGolem, [210.5, -59.0, 210.5]);
    if let Some(i) = p.s.mob_index(id)
        && let Some(m) = p.s.mobs.list[i].as_deref_mut()
    {
        m.golem.weather = 2;
        m.golem.next_weather = -2;
    }
    spawn(&mut p, Kind::SnowGolem, [212.5, -59.0, 210.5]);
    step(&mut p, 2);
    p.run("save\n");
    let list = super::super::tests::saved(&dir, 210 >> 4, 210 >> 4);
    let cu = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:copper_golem"));
    let sn = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:snow_golem"));
    eprintln!("copper {cu:?}");
    let cu = cu.expect("copper golem saved");
    assert_eq!(cu.get("weather_state").and_then(Nbt::as_str), Some("weathered"));
    assert_eq!(crate::nbtio::int(cu, "next_weather_age"), Some(-2));
    assert_eq!(crate::nbtio::int(sn.expect("snow golem saved"), "Pumpkin"), Some(1));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Anger read back: a golem angry at a cow stays angry at IT after the reload (target attached
/// by UUID), never at the neighboring player.
#[test]
fn iron_golem_anger_reloaded_keeps_its_target() {
    use super::super::mob::Ent;
    let Some((mut p, dir)) = probe("f06anger") else { return };
    let golem = spawn(&mut p, Kind::IronGolem, [216.5, -59.0, 206.5]);
    let cow = spawn(&mut p, Kind::Cow, [216.5, -59.0, 212.5]);
    let cow_uuid = p.s.mob_ref(cow).map(|c| c.uuid).unwrap();
    let time = p.s.time;
    let i = p.s.mob_index(golem).unwrap();
    let g = p.s.mobs.list[i].as_deref_mut().unwrap();
    g.golem.angry_at = Some(cow_uuid);
    g.anger_target = Some(Ent::Mob(cow));
    g.anger_end = time + 600;
    let e = g.save();
    g.load(&e);
    assert_eq!((g.anger_target, g.golem.angry_at), (None, Some(cow_uuid)));
    let mut seen = false;
    for _ in 0..60 {
        step(&mut p, 1);
        let g = p.s.mob_ref(golem).unwrap();
        assert_ne!(g.target, Some(Ent::Player), "golem turned against the player");
        assert_ne!(g.anger_target, Some(Ent::Player));
        seen |= g.anger_target == Some(Ent::Mob(cow));
    }
    assert!(seen, "anger not attached to the cow");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Obstruction on spawn by villagers: the game's "valid" spawn cells, full top face on the
/// ground, entities that block.
#[test]
fn iron_golem_spawn_obstruction() {
    let Some((mut p, dir)) = probe("f06obstr") else { return };
    let g = p.s.new_mob(Kind::IronGolem, [220.5, -59.0, 220.5], 0.0);
    assert!(p.s.golem_spawn_obstruction_ok(&g), "bare stone floor");
    p.run("setblock 220 -59 220 minecraft:white_carpet\n");
    assert!(p.s.golem_spawn_obstruction_ok(&g), "carpet at the feet accepted");
    p.run("setblock 220 -59 220 minecraft:water\n");
    assert!(p.s.golem_spawn_obstruction_ok(&g), "fluid at the feet ignored");
    p.run("setblock 220 -58 220 minecraft:water\n");
    assert!(!p.s.golem_spawn_obstruction_ok(&g), "fluid above refused");
    p.run("setblock 220 -58 220 minecraft:air\nsetblock 220 -59 220 minecraft:air\nsetblock 220 -58 220 minecraft:lever\n");
    assert!(!p.s.golem_spawn_obstruction_ok(&g), "signal source refused");
    p.run("setblock 220 -58 220 minecraft:air\nsetblock 220 -60 220 minecraft:stone_slab\n");
    assert!(!p.s.golem_spawn_obstruction_ok(&g), "bottom slab under the feet refused");
    p.run("setblock 220 -60 220 minecraft:stone\n");
    spawn(&mut p, Kind::Cow, [221.0, -59.0, 220.5]);
    assert!(!p.s.golem_spawn_obstruction_ok(&g), "cow in the box refused");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}
