//! G5-F04: combat and offensive enchantments (mace, thorns of mobs, credits, protections,
//! equipment modifiers of mobs, death message of a tamed animal, level values), on a copy of
//! the test world (`RUSTCRAFT_SOLO_WORLD`). Absent: tests skipped (failure under
//! `RUSTCRAFT_OPT_REQUIRE=1`).
#![cfg(test)]

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::SoundRef;

use crate::blocks::probe::Probe;
use crate::mobs::jrand::JRand;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;
use crate::player::Stack;

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [110.5, -59.0, 110.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    p.s.life.vit.cooldown = 0;
    p.s.yaw = 0.0;
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn st(id: &str) -> Stack {
    Stack { id: id.to_owned(), count: 1, components: None }
}

fn enchanted(id: &str, list: &[(&str, i32)]) -> Stack {
    let mut s = st(id);
    let fields = list.iter().map(|(e, l)| ((*e).to_owned(), Nbt::Int(*l))).collect();
    crate::items::set_component(&mut s, "minecraft:enchantments", Nbt::Compound(fields));
    s
}

fn wear(s: Option<&Stack>) -> i64 {
    s.and_then(|s| crate::items::component(s, "minecraft:damage")).and_then(Nbt::as_i64).unwrap_or(0)
}

/// A zombie with lots of health, not on the ground (smash in the air), facing the player.
fn tank(p: &mut Probe, pos: [f64; 3]) -> Mob {
    let mut z = p.s.new_mob(Kind::Zombie, pos, 180.0);
    z.attrs.max_health.base = 500.0;
    z.health = 500.0;
    z
}

fn sounds(p: &Probe, ev: &[Event], name: &str) -> usize {
    let Some(id) = p.s.sound_id(name) else { return 0 };
    ev.iter().filter(|e| matches!(e, Event::Sound(s) if s.sound == SoundRef::Id(id))).count()
}

/// Level-based values and value effects absent before: `exponent`, `exponential`; the binomial
/// removal beyond 128 trials draws one Gaussian (not one float per trial); fractional trial
/// counts are walked like the game (`y < n` in float).
#[test]
fn level_values_and_value_effects() {
    use serde_json::json;
    let v = json!({"type": "minecraft:exponent", "base": 2.0,
        "power": {"type": "minecraft:linear", "base": 1.0, "per_level_above_first": 1.0}});
    assert_eq!(crate::projectiles::ench::level_value(&v, 3), 8.0, "exponent");
    // 200 trials, p = 0.5: one Gaussian, rounded around 100, clamped.
    let mut a = JRand::new(42);
    let mut b = JRand::new(42);
    let out = crate::projectiles::ench::remove_binomial(0.5, 200.0, &mut a);
    let g = b.next_gaussian();
    let expect = 200.0 - crate::projectiles::ench::java_round_f64(100.0 + g * 50f64.sqrt()).clamp(0, 200) as f32;
    assert_eq!(out, expect, "normal approximation");
    assert_eq!(a.next_int(), b.next_int(), "more than one draw for the approximation");
    // 2.5 trials: three draws (0, 1, 2 < 2.5).
    let (mut a, mut b) = (JRand::new(7), JRand::new(7));
    crate::projectiles::ench::remove_binomial(0.0, 2.5, &mut a);
    for _ in 0..3 {
        b.next_float();
    }
    assert_eq!(a.next_int(), b.next_int(), "trial count of a fractional amount");
    let Some((p, dir)) = probe("g5f04-values") else { return };
    let e = json!({"type": "minecraft:exponential", "base": 2.0, "exponent": 3.0});
    let mut r = JRand::new(1);
    assert_eq!(p.s.value_effect(&e, 1, 1.5, &mut r), 12.0, "exponential");
    done(p, dir);
}

/// Levels set or merged are capped at 255; merging a level of 0 changes nothing.
#[test]
fn enchantment_level_capped() {
    let Some((p, dir)) = probe("g5f04-cap") else { return };
    let items = &p.s.life.items;
    let mut s = st("minecraft:diamond_sword");
    crate::enchanting::set_enchants(items, &mut s, &[("minecraft:sharpness".into(), 300)]);
    assert_eq!(crate::enchanting::enchants_of(items, &s), vec![("minecraft:sharpness".to_owned(), 255)]);
    let mut s = st("minecraft:diamond_sword");
    crate::enchanting::upgrade(items, &mut s, "minecraft:smite", 1000);
    assert_eq!(crate::enchanting::enchants_of(items, &s), vec![("minecraft:smite".to_owned(), 255)]);
    let mut s = st("minecraft:stick");
    crate::enchanting::upgrade(items, &mut s, "minecraft:smite", 0);
    assert!(s.components.is_none(), "level 0 wrote a component");
    done(p, dir);
}

/// A tamed wolf with an owner credits it (100 ticks: loot and experience "killed by a player");
/// tamed without an owner, the memory is forgotten; a wild wolf changes nothing.
#[test]
fn tamed_wolf_credits_owner() {
    let Some((mut p, dir)) = probe("g5f04-wolf") else { return };
    let owner = p.s.uuid;
    for (tame, owned, before, after) in [(true, true, 0, 100), (true, false, 50, 0), (false, false, 50, 50)] {
        let mut w = p.s.new_mob(Kind::Wolf, [112.5, -59.0, 112.5], 0.0);
        w.tame = tame;
        w.owner = owned.then_some(owner);
        let mut z = tank(&mut p, [112.5, -59.0, 113.5]);
        z.last_hurt_by_player = before;
        let id = z.id;
        p.s.add_mob(z);
        assert!(p.s.mob_attack(&mut w, Ent::Mob(id)), "wolf hit refused");
        let got = p.s.mob_ref(id).expect("zombie").last_hurt_by_player;
        assert_eq!(got, after, "tame {tame}, owned {owned}");
    }
    done(p, dir);
}

/// Protection of a mob's equipment: none against damage that ignores invulnerability (void), the
/// usual reduction otherwise (pack conditions).
#[test]
fn mob_protection_skips_void() {
    let Some((mut p, dir)) = probe("g5f04-prot") else { return };
    let boots = enchanted("minecraft:leather_boots", &[("minecraft:protection", 4)]);
    let loss = |p: &mut Probe, ty: &str, protected: bool| {
        let mut c = p.s.new_mob(Kind::Cow, [112.5, -59.0, 112.5], 0.0);
        if protected {
            c.equipment[2] = Some(boots.clone());
        }
        let h = c.health;
        assert!(p.s.mob_hurt(&mut c, ty, 4.0, None, None), "{ty} refused");
        h - c.health
    };
    assert_eq!(loss(&mut p, "minecraft:out_of_world", true), loss(&mut p, "minecraft:out_of_world", false), "void");
    let (with, without) = (loss(&mut p, "minecraft:generic", true), loss(&mut p, "minecraft:generic", false));
    assert!((with - without * 0.84).abs() < 1e-5, "protection IV: {with} for {without}");
    done(p, dir);
}

/// Equipment modifiers of a mob: a diamond chestplate gives 8 armor and 2 toughness (zombie base
/// armor 2), removed with the item; 10 damage → 10 × (1 − (10 − 10/2.5)/25) = 7.6. Fire Protection
/// IV boots: burning time × 0.4 (80 ticks of fire → 32).
#[test]
fn mob_equipment_modifiers() {
    let Some((mut p, dir)) = probe("g5f04-gear") else { return };
    let mut z = tank(&mut p, [112.5, -59.0, 112.5]);
    z.equipment[4] = Some(st("minecraft:diamond_chestplate"));
    p.s.mob_gear_tick(&mut z);
    assert_eq!(z.attrs.armor.value(), 10.0, "armor");
    assert_eq!(z.attrs.toughness.value(), 2.0, "toughness");
    let h = z.health;
    assert!(p.s.mob_hurt(&mut z, "minecraft:mob_attack", 10.0, None, None));
    assert!((h - z.health - 7.6).abs() < 1e-4, "absorbed damage {}", h - z.health);
    z.equipment[4] = None;
    p.s.mob_gear_tick(&mut z);
    assert_eq!((z.attrs.armor.value(), z.attrs.toughness.value()), (2.0, 0.0), "modifiers kept");
    assert!(z.gear_seen.is_none(), "tracking kept without equipment");
    z.equipment[2] = Some(enchanted("minecraft:leather_boots", &[("minecraft:fire_protection", 4)]));
    p.s.mob_gear_tick(&mut z);
    z.fire = 0;
    crate::inside_blocks::mob_ignite(&mut z, 80);
    assert_eq!(z.fire, 32, "burning time");
    done(p, dir);
}

/// A head item: damage that hurts helmets (falling anvil) loses a quarter; the cow has no armor.
#[test]
fn helmet_takes_a_quarter() {
    let Some((mut p, dir)) = probe("g5f04-helmet") else { return };
    let mut c = p.s.new_mob(Kind::Cow, [112.5, -59.0, 112.5], 0.0);
    c.equipment[5] = Some(st("minecraft:carved_pumpkin"));
    let h = c.health;
    assert!(p.s.mob_hurt(&mut c, "minecraft:falling_anvil", 8.0, None, None));
    assert_eq!(h - c.health, 6.0, "helmet reduction");
    done(p, dir);
}

/// A horse's body item with durability that wears on hurt loses ⌊8/4⌋ = 2 per hit; horse armor
/// does not wear on hurt (its equippable says so); a zombie's chestplate never wears from a hit.
#[test]
fn horse_armor_wears_mob_armor_does_not() {
    let Some((mut p, dir)) = probe("g5f04-horse") else { return };
    let mut h = p.s.new_mob(Kind::Horse, [112.5, -59.0, 112.5], 0.0);
    let durable = |id: &str, equippable: bool| {
        let mut s = st(id);
        crate::items::set_component(&mut s, "minecraft:max_damage", Nbt::Int(100));
        crate::items::set_component(&mut s, "minecraft:damage", Nbt::Int(0));
        if equippable {
            let eq = Nbt::Compound(vec![("slot".into(), Nbt::String("body".into()))]);
            crate::items::set_component(&mut s, "minecraft:equippable", eq);
        }
        s
    };
    h.equipment[6] = Some(durable("minecraft:iron_horse_armor", false));
    assert!(p.s.mob_hurt(&mut h, "minecraft:mob_attack", 8.0, None, None));
    assert_eq!(wear(h.equipment[6].as_ref()), 0, "horse armor worn on hurt");
    h.equipment[6] = Some(durable("minecraft:iron_horse_armor", true));
    h.damage_cooldown = 0;
    assert!(p.s.mob_hurt(&mut h, "minecraft:mob_attack", 8.0, None, None));
    assert_eq!(wear(h.equipment[6].as_ref()), 2, "horse body item wear");
    let mut z = tank(&mut p, [112.5, -59.0, 113.5]);
    z.equipment[4] = Some(st("minecraft:iron_chestplate"));
    assert!(p.s.mob_hurt(&mut z, "minecraft:mob_attack", 8.0, None, None));
    assert_eq!(wear(z.equipment[4].as_ref()), 0, "zombie armor worn by a hit");
    done(p, dir);
}

/// Thorns worn by mobs: the player who hits a zombie with thorns (level 10: always) takes 1 to 5
/// damage, the chestplate 2 points of wear; an iron golem that hits it gets hurt too; an arrow
/// shot by the player does the same.
#[test]
fn mob_thorns_hurt_attackers() {
    let Some((mut p, dir)) = probe("g5f04-thorns") else { return };
    p.s.set_difficulty(2, true);
    let thorny = enchanted("minecraft:iron_chestplate", &[("minecraft:thorns", 10)]);
    let mut z = tank(&mut p, [110.5, -59.0, 111.5]);
    z.equipment[4] = Some(thorny.clone());
    p.s.mobs.attack_ticker = 100;
    p.take_events();
    p.s.player_attack_mob(&mut z);
    let ev = p.take_events();
    let lost = 20.0 - p.s.life.vit.health;
    assert!((1.0..=5.0).contains(&lost), "player thorns damage {lost}");
    assert_eq!(wear(z.equipment[4].as_ref()), 2, "thorns wear");
    assert_eq!(sounds(&p, &ev, "minecraft:enchant.thorns.hit"), 1, "second hurt sound of thorns");
    // Iron golem.
    let mut g = p.s.new_mob(Kind::IronGolem, [110.5, -59.0, 113.0], 0.0);
    let gh = g.health;
    let mut z = tank(&mut p, [110.5, -59.0, 114.0]);
    z.equipment[4] = Some(thorny.clone());
    let id = z.id;
    p.s.add_mob(z);
    assert!(p.s.iron_golem_attack(&mut g, Ent::Mob(id)), "golem hit refused");
    assert!(g.health < gh, "golem not hurt by thorns");
    done(p, dir);
}

/// Wind Burst: the same after-hit engine as thorns; Fire Aspect of a sword reaches the mobs hit by
/// the sweep (and only those, with the weapon's bonus against each).
#[test]
fn sweep_carries_after_hit_effects() {
    let Some((mut p, dir)) = probe("g5f04-sweep") else { return };
    p.s.slots[36 + p.s.held as usize] = Some(enchanted("minecraft:diamond_sword", &[("minecraft:fire_aspect", 1)]));
    p.s.equipment_tick();
    p.s.on_ground = true;
    p.s.proj_player.known_move = [0.0; 3];
    p.s.mobs.attack_ticker = 100;
    let mut z = tank(&mut p, [110.5, -59.0, 112.0]);
    let side = tank(&mut p, [111.3, -59.0, 112.0]);
    let sid = side.id;
    p.s.add_mob(side);
    p.s.player_attack_mob(&mut z);
    assert!(z.fire > 0, "struck mob not ignited");
    let side = p.s.mob_ref(sid).expect("side");
    assert!(side.health < 500.0, "sweep missed");
    assert!(side.fire > 0, "swept mob not ignited");
    done(p, dir);
}

/// Mace smash (fall 4, in the air, full charge): 6 base + (12 + 2 × 1) by fall, critical × 1.5 =
/// 30, `mace_smash` damage, zombie armor 2: 30 × (1 − 0.4/25); air sound, level event 2013,
/// vertical speed 0.01, fall reset, 1 point of wear; a zombie within 3.5 blocks is pushed up by
/// 0.7.
#[test]
fn mace_smash_attack() {
    let Some((mut p, dir)) = probe("g5f04-mace") else { return };
    p.s.slots[36 + p.s.held as usize] = Some(st("minecraft:mace"));
    p.s.equipment_tick();
    p.s.on_ground = false;
    p.s.life.vit.fall = 4.0;
    p.s.mobs.attack_ticker = 100;
    let mut z = tank(&mut p, [110.5, -59.0, 111.5]);
    let near = tank(&mut p, [112.5, -59.0, 111.5]);
    let nid = near.id;
    p.s.add_mob(near);
    p.take_events();
    p.s.player_attack_mob(&mut z);
    let ev = p.take_events();
    let lost = 500.0 - z.health;
    assert!((lost - 30.0 * (1.0 - 0.4 / 25.0)).abs() < 1e-3, "smash damage {lost}");
    assert_eq!(z.last_hit.as_ref().map(|h| h.ty.as_str()), Some("minecraft:mace_smash"));
    assert_eq!(sounds(&p, &ev, "minecraft:item.mace.smash_air"), 1, "smash sound");
    assert!(ev.iter().any(|e| matches!(e, Event::LevelEvent { kind: 2013, data: 750, .. })), "level event");
    assert_eq!(p.s.mobs.player_vel[1], f64::from(0.01f32), "fall not stopped");
    assert_eq!(p.s.life.vit.fall, 0.0, "fall not reset");
    assert_eq!(wear(p.s.slots[36 + p.s.held as usize].as_ref()), 1, "mace wear");
    let n = p.s.mob_ref(nid).expect("near");
    assert!((n.vel[1] - f64::from(0.7f32)).abs() < 1e-9, "bystander not pushed: {:?}", n.vel);
    assert!(n.vel[0] > 0.0, "pushed away from the target");
    done(p, dir);
}

/// Density V adds 2.5 per fallen block; Breach IV lowers the armor fraction by 0.6 (diamond
/// chestplate: armor 10, toughness 2); Wind Burst I launches the player (explosion at its feet,
/// no wind charge grace) and pushes the struck zombie held outside the list.
#[test]
fn mace_enchantments() {
    let Some((mut p, dir)) = probe("g5f04-mace-ench") else { return };
    let hit = |p: &mut Probe, mace: Stack, chest: bool| {
        p.s.slots[36 + p.s.held as usize] = Some(mace);
        p.s.equipment_tick();
        p.s.on_ground = false;
        p.s.life.vit.fall = 4.0;
        p.s.mobs.attack_ticker = 100;
        p.s.mobs.player_vel = [0.0; 3];
        let mut z = tank(p, [110.5, -59.0, 111.5]);
        if chest {
            z.equipment[4] = Some(st("minecraft:diamond_chestplate"));
            p.s.mob_gear_tick(&mut z);
        }
        p.s.player_attack_mob(&mut z);
        (500.0 - z.health, z.vel)
    };
    let (dense, _) = hit(&mut p, enchanted("minecraft:mace", &[("minecraft:density", 5)]), false);
    assert!((dense - 45.0 * (1.0 - 0.4 / 25.0)).abs() < 1e-3, "density {dense}");
    let (plain, _) = hit(&mut p, st("minecraft:mace"), true);
    let (breach, _) = hit(&mut p, enchanted("minecraft:mace", &[("minecraft:breach", 4)]), true);
    let frac = (10.0f32 - 30.0 / 2.5).clamp(2.0, 20.0) / 25.0;
    assert!((plain - 30.0 * (1.0 - frac)).abs() < 1e-3, "armored {plain}");
    assert!((breach - 30.0 * (1.0 - (frac - 0.6).max(0.0))).abs() < 1e-3, "breach {breach}");
    let (_, still) = hit(&mut p, st("minecraft:mace"), false);
    assert_eq!(p.s.life.vit.impulse_grace, 40, "smash fall grace");
    let (_, zv) = hit(&mut p, enchanted("minecraft:mace", &[("minecraft:wind_burst", 1)]), false);
    assert!(p.s.mobs.player_vel[1] > 0.5, "player not launched: {:?}", p.s.mobs.player_vel);
    assert_eq!(p.s.life.vit.impulse_grace, 0, "wind burst kept the fall grace");
    assert!(zv[2] > still[2] + 0.1, "struck zombie not pushed: {zv:?} / {still:?}");
    done(p, dir);
}

/// A zombie holding a mace that falls 2 blocks smashes the player: `mace_smash` damage, 0.01
/// vertical speed, its fall is not reset (the game only resets a player's).
#[test]
fn mob_mace_smash() {
    let Some((mut p, dir)) = probe("g5f04-mob-mace") else { return };
    p.s.set_difficulty(2, true);
    let mut z = p.s.new_mob(Kind::Zombie, [110.5, -58.0, 111.0], 180.0);
    z.equipment[0] = Some(st("minecraft:mace"));
    p.s.mob_gear_tick(&mut z);
    z.fall_distance = 2.0;
    p.s.life.vit.health = 20.0;
    assert!(p.s.mob_attack(&mut z, Ent::Player), "zombie hit refused");
    assert_eq!(p.s.life.vit.last_damage.as_ref().map(|d| d.0.as_str()), Some("minecraft:mace_smash"));
    // Attack 3 + mace 5 + 8 by fall.
    assert!((20.0 - p.s.life.vit.health - 16.0).abs() < 1e-4, "damage {}", 20.0 - p.s.life.vit.health);
    assert_eq!(z.vel[1], f64::from(0.01f32), "fall not stopped");
    assert_eq!(z.fall_distance, 2.0, "mob fall reset");
    done(p, dir);
}

/// The owner receives the death message of its tamed wolf (rule on); rule off: nothing.
#[test]
fn tamed_animal_death_message() {
    let Some((mut p, dir)) = probe("g5f04-pet") else { return };
    for show in [true, false] {
        crate::nbtio::set(&mut p.s.rules, "minecraft:show_death_messages", Nbt::Byte(i8::from(show)));
        let mut w = p.s.new_mob(Kind::Wolf, [112.5, -59.0, 112.5], 0.0);
        w.tame = true;
        w.owner = Some(p.s.uuid);
        w.health = 1.0;
        let mut z = p.s.new_mob(Kind::Zombie, [112.5, -59.0, 113.5], 0.0);
        let id = w.id;
        p.s.add_mob(w);
        p.take_events();
        assert!(p.s.mob_attack(&mut z, Ent::Mob(id)), "zombie hit refused");
        let ev = p.take_events();
        let msg = ev.iter().find_map(|e| match e {
            Event::Chat { component, .. } if format!("{component:?}").contains("death.attack") => Some(format!("{component:?}")),
            _ => None,
        });
        if show {
            let m = msg.expect("no death message");
            assert!(m.contains("death.attack.mob") && m.contains("entity.minecraft.zombie"), "{m}");
        } else {
            assert!(msg.is_none(), "message with the rule off");
        }
    }
    done(p, dir);
}

/// Bench: equipment tracking of a mob every tick (nothing worn; full iron armor and sword, nothing
/// changed) and the after-hit effects of a hit without enchantments, and with a thorns chestplate
/// that never triggers (level 0 chance).
/// `cargo test --release -p rc-sim --lib bench_g5f04 -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_g5f04_gear_and_post_attack() {
    use std::hint::black_box;
    use std::time::Instant;
    let Some((mut p, dir)) = probe("g5f04-bench") else { return };
    let mut bare = tank(&mut p, [110.5, -59.0, 111.5]);
    let mut armored = tank(&mut p, [110.5, -59.0, 112.5]);
    for (k, id) in [
        (0, "minecraft:iron_sword"),
        (2, "minecraft:iron_boots"),
        (3, "minecraft:iron_leggings"),
        (4, "minecraft:iron_chestplate"),
        (5, "minecraft:iron_helmet"),
    ] {
        armored.equipment[k] = Some(st(id));
    }
    p.s.mob_gear_tick(&mut armored);
    let n = 2_000_000u32;
    for (name, m) in [("bare", &mut bare), ("armored", &mut armored)] {
        let t = Instant::now();
        for _ in 0..n {
            p.s.mob_gear_tick(black_box(&mut *m));
        }
        eprintln!("gear tick {name}: {:.1} ns", t.elapsed().as_nanos() as f64 / f64::from(n));
    }
    let n = 200_000u32;
    for (name, m) in [("bare", &mut bare), ("armored", &mut armored)] {
        let t = Instant::now();
        for _ in 0..n {
            let mut hit = crate::ench_fx::Hit {
                ty: "minecraft:player_attack",
                victim: crate::ench_fx::Side::Mob(&mut *m),
                attacker: Some(crate::ench_fx::Side::Player),
                direct: true,
            };
            p.s.post_attack(black_box(&mut hit), None);
        }
        eprintln!("post attack {name}: {:.1} ns", t.elapsed().as_nanos() as f64 / f64::from(n));
    }
    armored.equipment[4] = Some(enchanted("minecraft:iron_chestplate", &[("minecraft:thorns", 0)]));
    let t = Instant::now();
    for _ in 0..n {
        let mut hit = crate::ench_fx::Hit {
            ty: "minecraft:player_attack",
            victim: crate::ench_fx::Side::Mob(&mut armored),
            attacker: Some(crate::ench_fx::Side::Player),
            direct: true,
        };
        p.s.post_attack(black_box(&mut hit), None);
    }
    eprintln!("post attack thorns 0: {:.1} ns", t.elapsed().as_nanos() as f64 / f64::from(n));
    done(p, dir);
}

/// Generic protection (pack conditions) equals the former per-enchantment sum on the player:
/// Fire Protection III + Protection II against fire: 2 + 6 = 8 points → × 0.68.
#[test]
fn player_protection_from_pack() {
    let Some((mut p, dir)) = probe("g5f04-pprot") else { return };
    p.s.slots[8] = Some(enchanted("minecraft:leather_boots", &[("minecraft:fire_protection", 3)]));
    p.s.slots[5] = Some(enchanted("minecraft:leather_helmet", &[("minecraft:protection", 2)]));
    p.s.equipment_tick();
    let armor = p.s.armor_value() as f32;
    assert!(p.s.hurt("minecraft:in_fire", 5.0));
    let after_armor = 5.0 * (1.0 - (armor - 5.0 / 2.0).clamp(armor * 0.2, 20.0) / 25.0);
    let expect = 20.0 - after_armor * (1.0 - 8.0 / 25.0);
    assert!((p.s.life.vit.health - expect).abs() < 1e-4, "{} for {expect}", p.s.life.vit.health);
    done(p, dir);
}

fn adv(p: &Probe, id: &str) -> bool {
    let i = *p.s.prog.tree.index.get(id).unwrap_or_else(|| panic!("advancement {id} missing"));
    p.s.prog.done(i)
}

/// A falling player holding a mace, ready to smash.
fn ready_to_smash(p: &mut Probe, mace: Stack, fall: f64) {
    p.s.slots[36 + p.s.held as usize] = Some(mace);
    p.s.equipment_tick();
    p.s.on_ground = false;
    p.s.life.vit.fall = fall;
    p.s.mobs.attack_ticker = 100;
    p.s.mobs.player_vel = [0.0; 3];
}

/// The thorns of a still guardian answer any hit whose attacker is the direct entity: a mace
/// smash (type `mace_smash`) costs the player 2 HP.
#[test]
fn guardian_thorns_answer_mace_smash() {
    let Some((mut p, dir)) = probe("g5f04-guardian") else { return };
    p.s.set_difficulty(2, true);
    ready_to_smash(&mut p, st("minecraft:mace"), 4.0);
    let mut g = p.s.new_mob(Kind::Guardian, [110.5, -59.0, 111.5], 180.0);
    g.attrs.max_health.base = 500.0;
    g.health = 500.0;
    g.aqua.moving = false;
    p.s.player_attack_mob(&mut g);
    assert_eq!(g.last_hit.as_ref().map(|h| h.ty.as_str()), Some("minecraft:mace_smash"));
    let lost = 20.0 - p.s.life.vit.health;
    assert!((lost - 2.0).abs() < 1e-4, "guardian thorns: {lost} HP lost");
    done(p, dir);
}

/// `player_hurt_entity` sees the hit's real type: a Density V smash after a 25-block fall
/// (22 + 17 + 2.5 × 25 by fall, over 100 before armor) grants Over-Overkill.
#[test]
fn overoverkill_from_mace_smash() {
    let Some((mut p, dir)) = probe("g5f04-overoverkill") else { return };
    ready_to_smash(&mut p, enchanted("minecraft:mace", &[("minecraft:density", 5)]), 25.0);
    let mut z = tank(&mut p, [110.5, -59.0, 111.5]);
    assert!(!adv(&p, "minecraft:adventure/overoverkill"));
    p.s.player_attack_mob(&mut z);
    assert!(500.0 - z.health > 100.0, "smash too weak: {}", 500.0 - z.health);
    assert!(adv(&p, "minecraft:adventure/overoverkill"), "over-overkill not granted");
    done(p, dir);
}

/// Wing and bite hits of the dragon run the after-hit effects of the struck mob: a zombie wearing
/// thorns (level 10: always) wears its chestplate by 2; the thorns damage itself is refused by
/// the dragon (only a player's source or `always_hurts_ender_dragons` hurts it, like the game).
#[test]
fn dragon_bite_meets_mob_thorns() {
    use crate::mobs::end::phases::Phase;
    let Some((mut p, dir)) = probe("g5f04-dragon-thorns") else { return };
    let mut m = p.s.new_mob(Kind::EnderDragon, [110.5, -50.0, 110.5], 0.0);
    m.end.dragon.as_mut().expect("dragon").set_phase(Phase::HoldingPattern);
    let hp = m.health;
    let mut z = tank(&mut p, [110.5, -59.0, 112.5]);
    z.equipment[4] = Some(enchanted("minecraft:iron_chestplate", &[("minecraft:thorns", 10)]));
    let (zid, area) = (z.id, z.bbox());
    p.s.add_mob(z);
    let mut d = m.end.dragon.take().expect("dragon");
    p.s.dragon_bite(&mut m, &mut d, area);
    m.end.dragon = Some(d);
    let z = p.s.mob_ref(zid).expect("zombie");
    assert!(z.health < 500.0, "bite without effect");
    assert_eq!(wear(z.equipment[4].as_ref()), 2, "thorns wear");
    assert_eq!(m.health, hp, "a mob's thorns hurt the dragon");
    done(p, dir);
}

/// The spin attack of a trident is the player's full melee hit: a zombie wearing thorns (level
/// 10: always) touched by it hurts the player back and wears its chestplate.
#[test]
fn spin_attack_meets_mob_thorns() {
    let Some((mut p, dir)) = probe("g5f04-spin") else { return };
    p.s.set_difficulty(2, true);
    let trident = st("minecraft:trident");
    p.s.slots[36 + p.s.held as usize] = Some(trident.clone());
    p.s.equipment_tick();
    p.s.mobs.attack_ticker = 100;
    p.s.proj_player.spin = 20;
    p.s.proj_player.spin_damage = 8.0;
    p.s.proj_player.spin_item = Some(trident);
    let mut z = tank(&mut p, [110.5, -59.0, 110.9]);
    z.equipment[4] = Some(enchanted("minecraft:iron_chestplate", &[("minecraft:thorns", 10)]));
    let zid = z.id;
    p.s.add_mob(z);
    p.s.spin_attack_tick();
    assert_eq!(p.s.proj_player.spin, 0, "spin not stopped by the hit");
    let z = p.s.mob_ref(zid).expect("zombie");
    assert!(z.health < 500.0, "spin hit without damage");
    assert_eq!(wear(z.equipment[4].as_ref()), 2, "thorns wear");
    let lost = 20.0 - p.s.life.vit.health;
    assert!((1.0..=5.0).contains(&lost), "player thorns damage {lost}");
    done(p, dir);
}

/// Hit by its own projectile, the victim is its own attacker: its thorns (level 10: always) wear
/// the piece and hurt it, for the player (2 by thorns + 1 for the thorns damage it takes on its
/// armor) and for a mob (a mob's armor does not wear on a hit).
#[test]
fn own_projectile_meets_own_thorns() {
    let Some((mut p, dir)) = probe("g5f04-own-arrow") else { return };
    p.s.set_difficulty(2, true);
    let thorny = enchanted("minecraft:iron_chestplate", &[("minecraft:thorns", 10)]);
    p.s.slots[6] = Some(thorny.clone());
    p.s.post_attack_by(Ent::Player, Some(Ent::Player), "minecraft:arrow");
    assert_eq!(wear(p.s.slots[6].as_ref()), 3, "player's thorns wear");
    assert!(p.s.life.vit.health < 20.0, "player's thorns on itself");
    let mut z = tank(&mut p, [110.5, -59.0, 113.5]);
    z.equipment[4] = Some(thorny);
    let zid = z.id;
    p.s.add_mob(z);
    p.s.post_attack_by(Ent::Mob(zid), Some(Ent::Mob(zid)), "minecraft:arrow");
    let z = p.s.mob_ref(zid).expect("zombie");
    assert_eq!(wear(z.equipment[4].as_ref()), 2, "mob's thorns wear");
    assert!(z.health < 500.0, "mob's thorns on itself");
    done(p, dir);
}
