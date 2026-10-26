//! Spawning and finalisation (mission G4-F02) on a copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`, zone 170..262): spider group effect, zombie reinforcements,
//! Halloween pumpkin, zombie villager cure, drowned mount, nautilus breeding,
//! "structure" condition of variants, registered variants. World absent: tests skipped.

use rc_proto::nbt::Nbt;

use super::mob::{Ent, Mob};
use super::spawn::{Group, Reason, villager_type_name};
use super::species::Kind;
use crate::blocks::probe::Probe;
use crate::player::Stack;

/// Stone floor at −60, air above, player at (216.5, −59, 202.5), difficulty `diff`,
/// monster spawning allowed; local difficulty at maximum (clock and inhabited time).
fn probe(tag: &str, diff: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = super::tests::prep(&world, tag, 0, diff, &[]);
    let gp = dir.join("data").join("minecraft").join("game_rules.dat");
    let mut g = crate::nbtio::read_dat(&gp).unwrap_or(Nbt::Compound(Vec::new()));
    let mut d = g.get("data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
    for r in ["minecraft:spawn_mobs", "minecraft:spawn_monsters"] {
        crate::nbtio::set(&mut d, r, Nbt::Byte(1));
    }
    crate::nbtio::set(&mut g, "data", d);
    std::fs::write(&gp, crate::nbtio::gzip(&crate::nbtio::encode_root(&g))).unwrap();
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 170 -60 170 262 -60 262 minecraft:stone\nfill 170 -59 170 262 -54 262 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if crate::blocks::probe::state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 170..262 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.view = 8;
    // Midnight of a distant day: special multiplier at maximum, night for the rules.
    let t = 63 * 24_000 + 18_000;
    match p.s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
        Some(c) => c.1 = t,
        None => p.s.clocks.push(("minecraft:overworld".into(), t)),
    }
    p.s.time = t;
    for c in p.s.chunks.values_mut().flatten() {
        c.inhabited_time = 3_600_000;
    }
    Some((p, dir))
}

fn finish(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Finalised mob (not added to the world).
fn finalized(p: &mut Probe, k: Kind, pos: [f64; 3], reason: Reason, g: &mut Group) -> Mob {
    let mut m = p.s.new_mob(k, pos, 0.0);
    p.s.finalize_spawn(&mut m, reason, g);
    m
}

fn reg_index(p: &Probe, reg: &str, name: &str) -> i32 {
    p.s.regs.entries(reg).iter().position(|e| e == name).unwrap_or_else(|| panic!("{name} missing from {reg}")) as i32
}

/// Hard: the effect drawn for the group is applied, with infinite duration, to each spider of the
/// group; never in Normal.
#[test]
fn spider_group_effect() {
    let Some((mut p, dir)) = probe("g4f02spider", "hard") else { return };
    assert_eq!(p.s.local_difficulty([216, -59, 216]).1, 1.0, "special multiplier");
    let mut found = None;
    for _ in 0..500 {
        let mut g = Group::default();
        let a = finalized(&mut p, Kind::Spider, [216.5, -59.0, 216.5], Reason::Natural, &mut g);
        let Some(e) = g.spider_effect else { continue };
        let b = finalized(&mut p, Kind::Spider, [218.5, -59.0, 216.5], Reason::Natural, &mut g);
        found = Some((e, a, b));
        break;
    }
    let (e, a, b) = found.expect("no group effect in 500 groups (10 % expected)");
    assert!(["minecraft:speed", "minecraft:strength", "minecraft:regeneration", "minecraft:invisibility"].contains(&e));
    for m in [&a, &b] {
        let got = m.x.effects.iter().find(|x| x.id == e).expect("group effect missing");
        assert!(got.infinite() && got.amp == 0, "duration {} level {}", got.dur, got.amp);
    }
    finish(p, dir);
    let Some((mut p, dir)) = probe("g4f02spidern", "normal") else { return };
    for _ in 0..300 {
        let mut g = Group::default();
        let a = finalized(&mut p, Kind::Spider, [216.5, -59.0, 216.5], Reason::Natural, &mut g);
        assert!(g.spider_effect.is_none() && a.x.effects.is_empty(), "effect outside Hard");
    }
    finish(p, dir);
}

/// Chance of reinforcements drawn at finalisation (base < 0.1, registered; zombified piglin: 0);
/// zombie hurt in Hard with a chance of 1: a reinforcement zombie spawns, targets the
/// player, and both carry the 0.05 penalty.
#[test]
fn zombie_reinforcements() {
    let Some((mut p, dir)) = probe("g4f02reinf", "hard") else { return };
    let mut g = Group::default();
    let z = finalized(&mut p, Kind::Zombie, [216.5, -59.0, 216.5], Reason::SpawnEgg, &mut g);
    let r = z.attrs.reinforcements.as_ref().expect("reinforcement attribute");
    assert!((0.0..0.1).contains(&r.base) && r.saved, "base {}", r.base);
    let saved = p.s.mob_nbt(&z);
    let attrs = crate::nbtio::list(saved.get("attributes"));
    assert!(
        attrs.iter().any(|a| a.get("id").and_then(Nbt::as_str) == Some("minecraft:spawn_reinforcements")),
        "reinforcement attribute not registered"
    );
    let mut g = Group::default();
    let zp = finalized(&mut p, Kind::ZombifiedPiglin, [216.5, -59.0, 216.5], Reason::SpawnEgg, &mut g);
    assert_eq!(zp.attrs.reinforcements.as_ref().map(|a| a.base), Some(0.0));
    assert!(
        finalized(&mut p, Kind::Skeleton, [216.5, -59.0, 216.5], Reason::SpawnEgg, &mut g)
            .attrs
            .reinforcements
            .is_none()
    );

    let mut z = p.s.new_mob(Kind::Zombie, [216.5, -59.0, 220.5], 0.0);
    z.persistent = true;
    if let Some(r) = z.attrs.reinforcements.as_mut() {
        r.base = 1.0;
    }
    z.health = 1000.0;
    let id = z.id;
    p.s.add_mob(z);
    let count = |p: &Probe| p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Zombie && !m.removed).count();
    let before = count(&p);
    let mut called = false;
    for _ in 0..10 {
        let i = p.s.mob_index(id).unwrap();
        let mut m = p.s.mobs.list[i].take().unwrap();
        m.damage_cooldown = 0;
        let src = p.s.pos;
        p.s.mob_hurt(&mut m, "minecraft:player_attack", 1.0, Some(Ent::Player), Some(src));
        p.s.mobs.list[i] = Some(m);
        if count(&p) > before {
            called = true;
            break;
        }
    }
    assert!(called, "no reinforcement in 10 hits (chance 1, 50 position attempts per hit)");
    let n = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Zombie && m.id != id).unwrap();
    assert_eq!(n.target, Some(Ent::Player), "the reinforcement targets the player");
    assert_eq!(n.pos.map(f64::fract), [0.0; 3], "placed at integer coordinates");
    let callee = n.attrs.reinforcements.as_ref().unwrap();
    assert!(callee.mods.iter().any(|x| x.0 == "minecraft:reinforcement_callee_charge" && (x.1 + 0.05).abs() < 1e-6));
    let caller = p.s.mob_ref(id).unwrap().attrs.reinforcements.as_ref().unwrap();
    assert!(caller.mods.iter().any(|x| x.0 == "minecraft:reinforcement_caller_charge" && (x.1 + 0.05).abs() < 1e-9));
    finish(p, dir);
}

/// On 31 October: carved pumpkin or lantern on the head (zombie, skeleton, wither
/// skeleton), never dropped; another day: never.
#[test]
fn halloween_heads() {
    let Some((mut p, dir)) = probe("g4f02hallo", "normal") else { return };
    p.s.mobs.halloween = Some(true);
    for k in [Kind::Zombie, Kind::Skeleton, Kind::WitherSkeleton] {
        let (mut carved, mut lantern) = (0, 0);
        for _ in 0..600 {
            let mut g = Group::default();
            let m = finalized(&mut p, k, [216.5, -59.0, 216.5], Reason::Natural, &mut g);
            match m.equipment[5].as_ref().map(|s| s.id.as_str()) {
                Some("minecraft:carved_pumpkin") => carved += 1,
                Some("minecraft:jack_o_lantern") => lantern += 1,
                _ => continue,
            }
            assert_eq!(m.drop_chance[5], 0.0, "{k:?}: the Halloween head is not dropped");
        }
        // 25 % of empty heads, of which 10 % lanterns.
        assert!(carved > 60 && lantern > 0, "{k:?}: {carved} pumpkins, {lantern} lanterns out of 600");
    }
    p.s.mobs.halloween = Some(false);
    for _ in 0..200 {
        let mut g = Group::default();
        let m = finalized(&mut p, Kind::Zombie, [216.5, -59.0, 216.5], Reason::Natural, &mut g);
        assert!(!m.equipment[5].as_ref().is_some_and(|s| s.id.ends_with("pumpkin") || s.id.ends_with("lantern")));
    }
    finish(p, dir);
}

/// End of cure: the guaranteed-drop item is dropped, the cursed helmet goes into the
/// villager's inventory (slot 3), the rest disappears.
#[test]
fn cure_drops_preserved_equipment() {
    let Some((mut p, dir)) = probe("g4f02cure", "normal") else { return };
    let mut z = p.s.new_mob(Kind::ZombieVillager, [216.5, -59.0, 216.5], 0.0);
    let st = |id: &str| Stack { id: id.into(), count: 1, components: None };
    z.equipment[0] = Some(st("minecraft:iron_sword"));
    z.drop_chance[0] = 2.0;
    let mut helmet = st("minecraft:diamond_helmet");
    helmet.components = Some(Nbt::Compound(vec![(
        "minecraft:enchantments".into(),
        Nbt::Compound(vec![("minecraft:binding_curse".into(), Nbt::Int(1))]),
    )]));
    z.equipment[5] = Some(helmet);
    z.equipment[4] = Some(st("minecraft:iron_chestplate"));
    z.x.cure_time = 1;
    let drops_before = p.s.life.drops.len();
    assert!(p.s.variant_pre_tick(&mut z), "cure not completed");
    let new: Vec<&str> = p.s.life.drops[drops_before..].iter().map(|d| d.stack.id.as_str()).collect();
    assert_eq!(new, ["minecraft:iron_sword"], "items dropped");
    let v = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Villager).expect("villager");
    let inv = &v.npc.as_ref().expect("trader").inv;
    assert_eq!(inv[3].as_ref().map(|s| s.id.as_str()), Some("minecraft:diamond_helmet"));
    assert_eq!(inv.iter().flatten().count(), 1);
    finish(p, dir);
}

/// Natural (or structure) drowned with a trident: one time in two, mounted on an adult zombie
/// nautilus (persistent for a structure); never for an egg.
#[test]
fn drowned_rides_zombie_nautilus() {
    let Some((mut p, dir)) = probe("g4f02drown", "normal") else { return };
    for reason in [Reason::Natural, Reason::Structure] {
        let mut ridden = None;
        for _ in 0..3000 {
            let mut g = Group::default();
            let m = finalized(&mut p, Kind::Drowned, [216.5, -59.0, 216.5], reason, &mut g);
            // Baby drowned: chicken possible (zombie jockey); only the nautilus mount counts.
            if let Some(v) = m.mount.vehicle
                && p.s.mob_ref(v).is_some_and(|o| o.kind == Kind::ZombieNautilus)
            {
                ridden = Some((m, v));
                break;
            }
        }
        let (m, v) = ridden.unwrap_or_else(|| panic!("{reason:?}: no mounted drowned in 3,000 attempts"));
        assert!(m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:trident") && !m.is_baby());
        let n = p.s.mob_ref(v).expect("mount added to the world");
        assert_eq!(n.kind, Kind::ZombieNautilus);
        assert_eq!(n.persistent, reason == Reason::Structure);
        assert!(n.mount.passengers.contains(&Ent::Mob(m.id)));
    }
    for _ in 0..1500 {
        let mut g = Group::default();
        let m = finalized(&mut p, Kind::Drowned, [216.5, -59.0, 216.5], Reason::SpawnEgg, &mut g);
        let nautilus = m.mount.vehicle.and_then(|v| p.s.mob_ref(v)).is_some_and(|o| o.kind == Kind::ZombieNautilus);
        assert!(!nautilus, "mount outside natural or structure spawning");
    }
    finish(p, dir);
}

/// Birth of nautiluses put in love by the player: `animals_bred` statistic.
#[test]
fn nautilus_breeding_counts() {
    let Some((mut p, dir)) = probe("g4f02naut", "normal") else { return };
    let mut a = p.s.new_mob(Kind::Nautilus, [216.5, -59.0, 216.5], 0.0);
    let mut b = p.s.new_mob(Kind::Nautilus, [217.5, -59.0, 216.5], 0.0);
    for m in [&mut a, &mut b] {
        m.in_love = 600;
        m.love_cause = true;
    }
    let pid = b.id;
    p.s.add_mob(b);
    let before = p.s.prog.stats.get("minecraft:custom", "minecraft:animals_bred");
    p.s.nautilus_breed(&mut a, pid);
    assert_eq!(p.s.prog.stats.get("minecraft:custom", "minecraft:animals_bred"), before + 1);
    finish(p, dir);
}

/// Cat in a witch hut room (tag `cats_spawn_as_black`): always all black.
#[test]
fn cat_in_witch_hut_is_black() {
    let Some((mut p, dir)) = probe("g4f02cat", "normal") else { return };
    let pos = [216, -59, 216];
    let (cx, cz) = (pos[0] >> 4, pos[2] >> 4);
    let key = i64::from(cx) & 0xFFFF_FFFF | i64::from(cz) << 32;
    let c = p.s.chunks.get_mut(&(cx, cz)).and_then(|c| c.as_mut()).expect("chunk loaded");
    let start = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:swamp_hut".into())),
        (
            "Children".into(),
            Nbt::List(vec![Nbt::Compound(vec![("BB".into(), Nbt::IntArray(vec![210, -60, 210, 222, -50, 222]))])]),
        ),
    ]);
    let structures = Nbt::Compound(vec![
        ("References".into(), Nbt::Compound(vec![("minecraft:swamp_hut".into(), Nbt::LongArray(vec![key]))])),
        ("starts".into(), Nbt::Compound(vec![("minecraft:swamp_hut".into(), start)])),
    ]);
    crate::nbtio::set(&mut c.root, "structures", structures);
    let black = reg_index(&p, "minecraft:cat_variant", "minecraft:all_black");
    for _ in 0..30 {
        assert_eq!(p.s.select_variant("cat", pos), Some(black));
    }
    // Outside the room: the other coats remain possible.
    let others = (0..60).filter_map(|_| p.s.select_variant("cat", [216, -59, 240])).any(|v| v != black);
    assert!(others, "outside the hut, always black");
    finish(p, dir);
}

/// Villager types: exact table of the game (stony peaks: plains).
#[test]
fn villager_types_by_biome() {
    for (b, t) in [
        ("minecraft:stony_peaks", "plains"),
        ("minecraft:jagged_peaks", "snow"),
        ("minecraft:frozen_peaks", "snow"),
        ("minecraft:grove", "snow"),
        ("minecraft:cherry_grove", "plains"),
        ("minecraft:windswept_savanna", "savanna"),
        ("minecraft:windswept_forest", "taiga"),
        ("minecraft:snowy_taiga", "snow"),
        ("minecraft:mangrove_swamp", "swamp"),
        ("minecraft:eroded_badlands", "desert"),
        ("minecraft:deep_dark", "plains"),
        ("minecraft:ice_spikes", "snow"),
        ("minecraft:dripstone_caves", "plains"),
    ] {
        assert_eq!(villager_type_name(b), t, "{b}");
    }
}

/// Variants and sound variants: game defaults at construction, registered by
/// identifier and read back.
#[test]
fn variants_saved_and_defaults() {
    let Some((mut p, dir)) = probe("g4f02var", "normal") else { return };
    for (k, reg, def) in [
        (Kind::Pig, "pig", "minecraft:temperate"),
        (Kind::Cow, "cow", "minecraft:temperate"),
        (Kind::Chicken, "chicken", "minecraft:temperate"),
        (Kind::Wolf, "wolf", "minecraft:pale"),
        (Kind::Cat, "cat", "minecraft:black"),
    ] {
        let vreg = format!("minecraft:{reg}_variant");
        let sreg = format!("minecraft:{reg}_sound_variant");
        let m = p.s.new_mob(k, [216.5, -59.0, 216.5], 0.0);
        assert_eq!(m.variant, reg_index(&p, &vreg, def), "{k:?}: default variant");
        assert_eq!(m.sound_variant, reg_index(&p, &sreg, "minecraft:classic"), "{k:?}: default sound");
        // Last entry of each registry (never the default nor index 0).
        let mut m = m;
        m.variant = p.s.regs.entries(&vreg).len() as i32 - 1;
        m.sound_variant = p.s.regs.entries(&sreg).len() as i32 - 1;
        let e = p.s.mob_nbt(&m);
        let vname = p.s.regs.entries(&vreg).last().cloned().unwrap();
        let sname = p.s.regs.entries(&sreg).last().cloned().unwrap();
        assert_eq!(e.get("variant").and_then(Nbt::as_str), Some(vname.as_str()), "{k:?}");
        assert_eq!(e.get("sound_variant").and_then(Nbt::as_str), Some(sname.as_str()), "{k:?}");
        let id = p.s.next_entity;
        p.s.next_entity += 1;
        let got = p.s.adopt_with_passengers(id, &e, None).expect("mob read back");
        let back = p.s.mob_ref(got).unwrap();
        assert_eq!((back.variant, back.sound_variant), (m.variant, m.sound_variant), "{k:?} read back");
    }
    // Identifier without namespace accepted; unknown: default.
    let pig = p.s.new_mob(Kind::Pig, [216.5, -59.0, 216.5], 0.0);
    let mut e = p.s.mob_nbt(&pig);
    crate::nbtio::set(&mut e, "variant", Nbt::String("warm".into()));
    crate::nbtio::set(&mut e, "sound_variant", Nbt::String("minecraft:nope".into()));
    crate::nbtio::remove(&mut e, "UUID");
    let id = p.s.next_entity;
    p.s.next_entity += 1;
    let got = p.s.adopt_with_passengers(id, &e, None).expect("pig read back");
    let back = p.s.mob_ref(got).unwrap();
    assert_eq!(back.variant, reg_index(&p, "minecraft:pig_variant", "minecraft:warm"));
    assert_eq!(back.sound_variant, reg_index(&p, "minecraft:pig_sound_variant", "minecraft:classic"));
    finish(p, dir);
}

/// Proofs with exact random (fingerprints of `tests_opt`, bench) do not depend on the date:
/// in tests, Halloween day is frozen to "no" from construction.
#[test]
fn halloween_pinned_in_tests() {
    assert_eq!(super::Mobs::default().halloween, Some(false));
}

/// Monster spawner whose data carries a variant: the mob is born with it (read like
/// the NBT, without finalisation).
#[test]
fn spawner_keeps_saved_variant() {
    let Some((mut p, dir)) = probe("g4f02spawner", "normal") else { return };
    p.run("fill 206 -60 206 226 -60 226 minecraft:grass_block\nsetblock 216 -59 216 minecraft:spawner\n");
    let cold = reg_index(&p, "minecraft:pig_variant", "minecraft:cold");
    let at = [216, -59, 216];
    let entity = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:pig".into())),
        ("variant".into(), Nbt::String("minecraft:cold".into())),
    ]);
    let players = [p.s.pos];
    for _ in 0..20 {
        let be = p.s.be_get_mut(at).expect("spawner placed");
        crate::nbtio::set(be, "SpawnData", Nbt::Compound(vec![("entity".into(), entity.clone())]));
        crate::nbtio::set(be, "Delay", Nbt::Short(0));
        p.s.spawner_tick(at, &players);
        if p.s.mobs.list.iter().flatten().any(|m| m.kind == Kind::Pig) {
            break;
        }
    }
    let pigs: Vec<i32> = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Pig).map(|m| m.variant).collect();
    assert!(!pigs.is_empty(), "no pig in 20 passes of the spawner");
    assert!(pigs.iter().all(|&v| v == cold), "variants {pigs:?}, cold = {cold}");
    finish(p, dir);
}

/// `data` on an existing mob: absent or unknown variant = current variant kept.
#[test]
fn data_keeps_variant_when_absent_or_unknown() {
    let Some((mut p, dir)) = probe("g4f02data", "normal") else { return };
    let cold = reg_index(&p, "minecraft:pig_variant", "minecraft:cold");
    let big = p.s.regs.entries("minecraft:pig_sound_variant").len() as i32 - 1;
    let mut pig = p.s.new_mob(Kind::Pig, [216.5, -59.0, 216.5], 0.0);
    pig.variant = cold;
    pig.sound_variant = big;
    let mut e = p.s.mob_nbt(&pig);
    crate::nbtio::remove(&mut e, "variant");
    crate::nbtio::set(&mut e, "sound_variant", Nbt::String("minecraft:nope".into()));
    let id = pig.id;
    p.s.add_mob(pig);
    p.s.ent_set_nbt(crate::commands::ents::E::Mob(id), &e);
    let m = p.s.mob_ref(id).unwrap();
    assert_eq!((m.variant, m.sound_variant), (cold, big));
    finish(p, dir);
}

/// A dying mob (0 HP, not yet removed) and the dead player
/// block the space like living ones; the spectator does not.
#[test]
fn dying_entities_still_obstruct() {
    let Some((mut p, dir)) = probe("g4f02obst", "normal") else { return };
    let mut z = p.s.new_mob(Kind::Zombie, [210.5, -59.0, 210.5], 0.0);
    z.health = 0.0;
    let bb = z.bbox();
    p.s.add_mob(z);
    assert!(!p.s.unobstructed(&bb, -1), "dying mob ignored");
    let pb = p.s.player_aabb();
    p.s.life.vit.dead = true;
    p.s.life.vit.health = 0.0;
    assert!(!p.s.unobstructed(&pb, -1), "dead player ignored");
    p.s.game_mode = 3;
    assert!(p.s.unobstructed(&pb, -1), "spectator counted");
    finish(p, dir);
}

/// Bench: cost of the local date read at each finalisation of a zombie or skeleton
/// (`cargo test --release -p rc-sim bench_local_date -- --ignored --nocapture`).
#[test]
#[ignore]
fn bench_local_date() {
    let n = 200_000;
    let t = std::time::Instant::now();
    let mut acc = 0u32;
    for _ in 0..n {
        let (_, mo, d, ..) = crate::storage::local_datetime(crate::progress::now_ms());
        acc = acc.wrapping_add(mo + d);
    }
    let ns = t.elapsed().as_nanos() as f64 / f64::from(n);
    eprintln!("local date: {ns:.0} ns per read ({acc})");
}
