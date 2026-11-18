//! L9-COMBAT: limits of combat, entities and commands. The smash knockback and the wind bursts
//! reach armor stands, mannequins, minecarts and End crystals; `player_hurt_entity` fires for
//! every hurt whose source is the player; `/attribute` reads armor stands and mannequins;
//! `/summon` writes their living fields; `/fetchprofile` goes through the profile cache. On a
//! copy of the test world (`RUSTCRAFT_SOLO_WORLD`). Absent: tests skipped (failure under
//! `RUSTCRAFT_OPT_REQUIRE=1`).
#![cfg(test)]

use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::commands::ents::E;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::nbtio;
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

fn st(id: &str) -> Stack {
    Stack { id: id.to_owned(), count: 1, components: None }
}

/// A zombie with lots of health, facing the player.
fn tank(p: &mut Probe, pos: [f64; 3]) -> Mob {
    let mut z = p.s.new_mob(Kind::Zombie, pos, 180.0);
    z.attrs.max_health.base = 500.0;
    z.health = 500.0;
    z
}

/// A falling player holding a mace, ready to smash.
fn ready_to_smash(p: &mut Probe, fall: f64) {
    p.s.slots[36 + p.s.held as usize] = Some(st("minecraft:mace"));
    p.s.equipment_tick();
    p.s.on_ground = false;
    p.s.life.vit.fall = fall;
    p.s.mobs.attack_ticker = 100;
    p.s.mobs.player_vel = [0.0; 3];
}

/// Entity summoned with `/summon`'s path; its network identifier.
fn summon(p: &mut Probe, ty: &str, pos: [f64; 3], nbt: &str) -> i32 {
    let src = p.s.player_source();
    let n = crate::commands::snbt::parse(nbt).expect("snbt");
    match p.s.summon_entity(&src, ty, pos, Some(n)) {
        Ok(E::Other(id)) => id,
        r => panic!("summon {ty}: {r:?}"),
    }
}

/// Saved motion of a decoration.
fn motion(p: &Probe, id: i32) -> [f64; 3] {
    nbtio::motion(p.s.deco_nbt(id).expect("decoration").get("Motion")).unwrap_or_default()
}

/// The smash knockback pushes the armor stands and mannequins around the target like any living
/// bystander: (3.5 − 2) × 0.7 away from it and 0.7 upwards, reduced by the knockback resistance of
/// a netherite chestplate worn by the stand (0.1); a marker stand is left alone.
#[test]
fn smash_pushes_stands_and_mannequins() {
    let Some((mut p, _dir)) = probe("l9-smash-deco") else { return };
    ready_to_smash(&mut p, 4.0);
    let mut z = tank(&mut p, [110.5, -59.0, 111.5]);
    let stand = summon(&mut p, "minecraft:armor_stand", [112.5, -59.0, 111.5], "{NoGravity:1b}");
    let heavy = summon(
        &mut p,
        "minecraft:armor_stand",
        [108.5, -59.0, 111.5],
        "{NoGravity:1b,equipment:{chest:{id:\"minecraft:netherite_chestplate\",count:1}}}",
    );
    let marker = summon(&mut p, "minecraft:armor_stand", [110.5, -59.0, 113.0], "{NoGravity:1b,Marker:1b}");
    let man = summon(&mut p, "minecraft:mannequin", [110.5, -59.0, 113.5], "{NoGravity:1b}");
    p.s.player_attack_mob(&mut z);
    let up = f64::from(0.7f32);
    let power = (3.5 - 2.0) * f64::from(0.7f32);
    let m = motion(&p, stand);
    assert_eq!(m, [power, up, 0.0], "stand not pushed like a living bystander");
    let h = motion(&p, heavy);
    // The armor's resistance is a float widened, like the game's material value.
    let res = f64::from(0.1f32);
    assert_eq!(h, [-(3.5 - 2.0) * f64::from(0.7f32) * (1.0 - res), up, 0.0], "knockback resistance of the stand's armor");
    assert_eq!(motion(&p, marker), [0.0; 3], "a marker is not pushed");
    assert_eq!(motion(&p, man)[1], up, "mannequin not pushed");
    assert!(motion(&p, man)[2] > 0.0, "mannequin pushed away from the target");
}

/// A weapon's Wind Burst (no source entity, the mob rule on) pushes a visible armor stand, a
/// mannequin, a minecart and an End crystal (whose saved motion keeps it); an invisible stand is
/// not reached. The burst of a wind charge itself no longer reaches the stand (block-like), but
/// still the minecart.
#[test]
fn wind_burst_pushes_stands_minecarts_crystals() {
    let Some((mut p, _dir)) = probe("l9-wind-burst") else { return };
    let stand = summon(&mut p, "minecraft:armor_stand", [112.5, -59.0, 110.5], "{NoGravity:1b}");
    let ghost = summon(&mut p, "minecraft:armor_stand", [108.5, -59.0, 110.5], "{NoGravity:1b,Invisible:1b}");
    let man = summon(&mut p, "minecraft:mannequin", [110.5, -59.0, 108.0], "{NoGravity:1b}");
    let cart = summon(&mut p, "minecraft:minecart", [110.5, -59.0, 112.5], "{}");
    let crystal = p.s.spawn_crystal([108.5, -59.0, 108.5], 0.0, false);
    p.s.pos = [130.5, -59.0, 130.5];
    let center = [110.5, -58.5, 110.5];
    let sound = "minecraft:entity.wind_charge.wind_burst";
    p.s.wind_burst_with(None, None, center, 3.5, 1.2, sound, &mut []);
    assert!(motion(&p, stand)[0] > 0.0, "stand not pushed: {:?}", motion(&p, stand));
    assert_eq!(motion(&p, ghost), [0.0; 3], "an invisible stand ignores the burst");
    assert!(motion(&p, man)[2] < 0.0, "mannequin not pushed: {:?}", motion(&p, man));
    let v = p.s.cart_ref(cart).expect("minecart").vel;
    assert!(v[2] > 0.0, "minecart not pushed: {v:?}");
    let c = p.s.mobs.end.crystals.iter().find(|c| c.id == crystal).expect("crystal");
    assert!(c.vel[0] < 0.0 && c.vel[2] < 0.0, "crystal not pushed: {:?}", c.vel);
    let saved = nbtio::motion(c.save().get("Motion")).unwrap_or_default();
    assert_eq!(saved, c.vel, "pushed velocity of the crystal not saved");
    // A wind charge's own burst: block-like entities are not reached.
    let before = motion(&p, stand);
    let src = crate::mobs::recent::vibration::Src::Projectile(None);
    p.s.wind_burst_with(Some(src), None, center, 3.5, 1.2, sound, &mut []);
    assert_eq!(motion(&p, stand), before, "a wind charge pushed an armor stand");
    assert!(p.s.cart_ref(cart).expect("minecart").vel[2] > v[2], "a wind charge spared the minecart");
}

/// Wind Burst under the mob rule knocks an item frame off (a non-zero push breaks what is hung,
/// its item dropped); a wind charge's own burst leaves it.
#[test]
fn wind_burst_knocks_frame_off() {
    let Some((mut p, _dir)) = probe("l9-wind-frame") else { return };
    let src = p.s.player_source();
    assert!(p.s.perform(src, "summon minecraft:item_frame 110.5 -59.0 110.5 {Facing:1b,block_pos:[I;110,-59,110]}").0);
    let count = |p: &Probe| p.s.all_ents().into_iter().filter(|e| p.s.ent_type(*e) == "minecraft:item_frame").count();
    assert_eq!(count(&p), 1, "frame not placed");
    let center = [110.5, -58.5, 111.0];
    let sound = "minecraft:entity.wind_charge.wind_burst";
    let charge = crate::mobs::recent::vibration::Src::Projectile(None);
    p.s.wind_burst_with(Some(charge), None, center, 1.2, 1.22, sound, &mut []);
    assert_eq!(count(&p), 1, "a wind charge broke the frame");
    p.s.wind_burst_with(None, None, center, 3.5, 1.2, sound, &mut []);
    assert_eq!(count(&p), 0, "Wind Burst left the frame in place");
    assert!(p.s.life.drops.iter().any(|d| !d.is_removed() && d.stack.id == "minecraft:item_frame"), "frame item not dropped");
}

/// Advancement tree with one criterion `player_hurt_entity` on a husk.
fn husk_hurt_tree(p: &mut Probe) {
    let mut raw = std::collections::BTreeMap::new();
    raw.insert(
        "l9:hurt_husk".to_owned(),
        serde_json::json!({"criteria": {"hit": {"trigger": "minecraft:player_hurt_entity",
            "conditions": {"entity": {"minecraft:entity_type": "minecraft:husk"}}}}}),
    );
    p.s.prog.reload(std::sync::Arc::new(crate::progress::Tree::build(raw)));
}

fn done(p: &Probe, id: &str) -> bool {
    p.s.prog.tree.index.get(id).is_some_and(|&i| p.s.prog.done(i))
}

/// The sweep hurts the husk standing next to the main target: `player_hurt_entity` fires for it
/// too (the game emits it from every hurt whose source is the player, not only for the target).
#[test]
fn sweep_victim_triggers_player_hurt_entity() {
    let Some((mut p, _dir)) = probe("l9-sweep-trigger") else { return };
    husk_hurt_tree(&mut p);
    p.s.slots[36 + p.s.held as usize] = Some(st("minecraft:diamond_sword"));
    p.s.equipment_tick();
    p.s.on_ground = true;
    p.s.mobs.attack_ticker = 100;
    p.s.life.vit.sprinting = false;
    p.s.life.vit.fall = 0.0;
    p.s.proj_player.known_move = [0.0; 3];
    let mut z = tank(&mut p, [110.5, -59.0, 111.5]);
    let mut h = p.s.new_mob(Kind::Husk, [111.3, -59.0, 111.5], 180.0);
    h.attrs.max_health.base = 500.0;
    h.health = 500.0;
    let hid = h.id;
    p.s.add_mob(h);
    assert!(p.s.trig_active("minecraft:player_hurt_entity"), "test criterion not loaded");
    assert!(!done(&p, "l9:hurt_husk"));
    p.s.player_attack_mob(&mut z);
    assert!(z.health < 500.0, "main hit without damage");
    assert!(p.s.mob_ref(hid).is_some_and(|m| m.health < 500.0), "sweep missed the husk");
    assert!(done(&p, "l9:hurt_husk"), "the sweep's victim did not trigger player_hurt_entity");
}

/// An arrow of the player that hurts a mob grants "Take Aim": the trigger sees the arrow as the
/// direct entity (before, the direct entity was unknown and the advancement never came).
#[test]
fn arrow_hit_grants_take_aim() {
    use crate::projectiles::physics::{EntityHit, Target};
    use crate::projectiles::{PKind, Proj};
    let Some((mut p, _dir)) = probe("l9-take-aim") else { return };
    let z = tank(&mut p, [114.5, -59.0, 110.5]);
    let (zid, pos) = (z.id, z.pos());
    p.s.add_mob(z);
    let mut arrow = Proj::new(PKind::Arrow, 900_001, [112.5, -58.5, 110.5], st("minecraft:arrow"));
    arrow.owner = Some(p.s.uuid);
    arrow.damage = 2.0;
    arrow.vel = [3.0, 0.0, 0.0];
    assert!(!done(&p, "minecraft:adventure/shoot_arrow"));
    p.s.proj_hit_entity(&mut arrow, EntityHit { ent: Target::Living(crate::mobs::mob::Ent::Mob(zid)), loc: pos });
    assert!(p.s.mob_ref(zid).is_some_and(|m| m.health < 500.0), "arrow without damage");
    assert!(done(&p, "minecraft:adventure/shoot_arrow"), "Take Aim not granted by a landed arrow");
    assert!(p.s.proj.direct_hit.is_none(), "direct projectile left behind");
}

/// The player's hurt by a falling block: a worn helmet wears (⌊10/4⌋ for the hit, then 1 with the
/// other pieces) and the damage loses a quarter, like any living entity; `entity_hurt_player` sees
/// the damage received (10) and the damage after the helmet, before armor (7.5), not the health
/// lost.
#[test]
fn falling_block_meets_player_helmet() {
    let Some((mut p, _dir)) = probe("l9-helmet") else { return };
    let mut raw = std::collections::BTreeMap::new();
    raw.insert(
        "l9:hurt_player".to_owned(),
        serde_json::json!({"criteria": {"hit": {"trigger": "minecraft:entity_hurt_player",
            "conditions": {"damage": {"dealt": {"min": 9.9, "max": 10.1}, "taken": {"min": 7.4, "max": 7.6}}}}}}),
    );
    p.s.prog.reload(std::sync::Arc::new(crate::progress::Tree::build(raw)));
    p.s.slots[5] = Some(st("minecraft:iron_helmet"));
    p.s.slots[6] = Some(st("minecraft:diamond_chestplate"));
    p.s.equipment_tick();
    assert!(p.s.hurt("minecraft:falling_block", 10.0), "hit refused");
    let worn = |s: Option<&Stack>| s.and_then(|s| crate::items::component(s, "minecraft:damage")).and_then(Nbt::as_i64).unwrap_or(0);
    assert_eq!(worn(p.s.slots[5].as_ref()), 3, "helmet wear");
    assert_eq!(worn(p.s.slots[6].as_ref()), 1, "chestplate wear");
    assert!(done(&p, "l9:hurt_player"), "entity_hurt_player did not see 10 received and 7.5 before armor");
}

/// Ranged hits do not count in the `damage_dealt` statistic (the game only counts the melee
/// hits of the player).
#[test]
fn arrow_hit_is_not_damage_dealt() {
    use crate::projectiles::physics::{EntityHit, Target};
    use crate::projectiles::{PKind, Proj};
    let Some((mut p, _dir)) = probe("l9-arrow-stat") else { return };
    let z = tank(&mut p, [114.5, -59.0, 110.5]);
    let (zid, pos) = (z.id, z.pos());
    p.s.add_mob(z);
    let mut arrow = Proj::new(PKind::Arrow, 900_002, [112.5, -58.5, 110.5], st("minecraft:arrow"));
    arrow.owner = Some(p.s.uuid);
    arrow.damage = 2.0;
    arrow.vel = [3.0, 0.0, 0.0];
    p.s.proj_hit_entity(&mut arrow, EntityHit { ent: Target::Living(crate::mobs::mob::Ent::Mob(zid)), loc: pos });
    assert!(p.s.mob_ref(zid).is_some_and(|m| m.health < 500.0), "arrow without damage");
    assert_eq!(p.s.stat_value("minecraft:custom", "minecraft:damage_dealt"), 0, "ranged damage counted as dealt");
}

/// Bench (ignored): cost of a Wind Burst (no source, mob rule on) with 0 then 400 armor stands
/// around it (a quarter in range), and of the deco pass alone. `cargo test --release bench_wind_burst
/// -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_wind_burst_decorations() {
    let Some((mut p, _dir)) = probe("l9-bench-wind") else { return };
    let center = [110.5, -58.5, 110.5];
    let sound = "minecraft:entity.wind_charge.wind_burst";
    let time = |p: &mut Probe, n: u32, f: &dyn Fn(&mut Probe)| {
        let t = std::time::Instant::now();
        for _ in 0..n {
            f(p);
        }
        t.elapsed().as_secs_f64() * 1e6 / f64::from(n)
    };
    let burst = |p: &mut Probe| p.s.wind_burst_with(None, None, center, 3.5, 1.2, sound, &mut []);
    let pass = |p: &mut Probe| p.s.deco_blast_push(center, 3.5, 1.2, true);
    let empty = (time(&mut p, 50, &burst), time(&mut p, 50, &pass));
    for i in 0..400 {
        let (x, z) = (100.5 + f64::from(i % 20) * 1.5, 100.5 + f64::from(i / 20) * 1.5);
        summon(&mut p, "minecraft:armor_stand", [x, -59.0, z], "{NoGravity:1b}");
    }
    let full = (time(&mut p, 50, &burst), time(&mut p, 50, &pass));
    eprintln!("wind burst: 0 stands {:.1} us (deco pass {:.2} us); 400 stands {:.1} us (deco pass {:.1} us)", empty.0, empty.1, full.0, full.1);
    // Split of the deco pass: scan alone (nothing in range), visible part of the stands in range.
    let scan = time(&mut p, 50, &|p: &mut Probe| p.s.deco_blast_push([0.5, -200.0, 0.5], 0.5, 1.2, true));
    let near: Vec<rc_physics::Aabb> =
        p.s.deco_living().into_iter().filter(|d| crate::mobs::mth::dist2(d.pos, center) <= 49.0).map(|d| d.bbox).collect();
    let seen = time(&mut p, 50, &|p: &mut Probe| {
        for b in &near {
            std::hint::black_box(p.s.seen_percent(center, b));
        }
    });
    eprintln!("deco pass split: scan {scan:.1} us, visible part of {} stands {seen:.1} us", near.len());
}

fn nbt_of(p: &Probe, id: i32) -> Nbt {
    p.s.deco_nbt(id).cloned().expect("entity")
}

/// Command at operator level; its success and result.
fn cmd(p: &mut Probe, c: &str) -> (bool, i32) {
    let mut src = p.s.player_source();
    src.level = 4;
    p.s.perform(src, c)
}

/// `/attribute` reads armor stands and mannequins like any living entity: the attributes of
/// their type (a stand steps 0, a mannequin 0.6; no attack damage), their saved base and
/// modifiers, the modifiers of their worn equipment; a set base is saved in their compound. The
/// player's table is the game's (camera distance and the other attributes of 26.3).
#[test]
fn attribute_reads_stands_and_mannequins() {
    let Some((mut p, _dir)) = probe("l9-attribute") else { return };
    let stand = summon(&mut p, "minecraft:armor_stand", [112.5, -59.0, 110.5], "{NoGravity:1b,Tags:[\"a\"]}");
    let _heavy = summon(
        &mut p,
        "minecraft:armor_stand",
        [114.5, -59.0, 110.5],
        "{NoGravity:1b,Tags:[\"h\"],equipment:{chest:{id:\"minecraft:netherite_chestplate\",count:1}}}",
    );
    let _man = summon(&mut p, "minecraft:mannequin", [116.5, -59.0, 110.5], "{NoGravity:1b,Tags:[\"m\"]}");
    let a = "@e[tag=a,limit=1]";
    assert_eq!(cmd(&mut p, &format!("attribute {a} minecraft:max_health get")), (true, 20), "stand health");
    assert_eq!(cmd(&mut p, &format!("attribute {a} minecraft:step_height get 10")), (true, 0), "stand step");
    assert_eq!(cmd(&mut p, &format!("attribute {a} minecraft:movement_speed get 10")), (true, 7), "living default speed 0.7");
    assert!(!cmd(&mut p, &format!("attribute {a} minecraft:attack_damage get")).0, "a stand has no attack damage");
    assert_eq!(cmd(&mut p, "attribute @e[tag=m,limit=1] minecraft:step_height get 10"), (true, 6), "mannequin step");
    assert_eq!(cmd(&mut p, "attribute @e[tag=h,limit=1] minecraft:armor get"), (true, 8), "armor of the worn chestplate");
    assert_eq!(
        cmd(&mut p, "attribute @e[tag=h,limit=1] minecraft:knockback_resistance get 1000"),
        (true, 100),
        "knockback resistance of the netherite chestplate"
    );
    // Reset without an entry of its own: nothing saved.
    assert!(cmd(&mut p, &format!("attribute {a} minecraft:scale base reset")).0);
    assert!(nbtio::list(nbt_of(&p, stand).get("attributes")).is_empty(), "a reset created an entry");
    assert!(cmd(&mut p, &format!("attribute {a} minecraft:scale base set 2")).0);
    assert!(cmd(&mut p, &format!("attribute {a} minecraft:scale modifier add l9:big 0.5 add_multiplied_base")).0);
    assert_eq!(cmd(&mut p, &format!("attribute {a} minecraft:scale get 10")), (true, 30), "base 2 + 50 %");
    let e = nbt_of(&p, stand);
    let saved = crate::attrs::saved_entry(nbtio::list(e.get("attributes")), "minecraft:scale");
    assert_eq!(saved, Some((Some(2.0), vec![("l9:big".to_owned(), 0.5, 1)])), "scale not saved in the stand");
    assert!(cmd(&mut p, &format!("attribute {a} minecraft:scale modifier remove l9:big")).0);
    assert!(!cmd(&mut p, &format!("attribute {a} minecraft:scale modifier remove l9:big")).0, "removed twice");
    // The player: attributes of 26.3 the old table did not know.
    assert_eq!(cmd(&mut p, "attribute @s minecraft:camera_distance get"), (true, 4), "player camera distance");
    assert_eq!(cmd(&mut p, "attribute @s minecraft:name_tag_distance get"), (true, 64), "player name tag distance");
    assert!(!cmd(&mut p, "attribute @s minecraft:flying_speed get").0, "a player has no flying speed");
}

/// Keys of a compound, in order.
fn keys(n: &Nbt) -> Vec<String> {
    match n {
        Nbt::Compound(f) => f.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    }
}

/// `/summon` of an armor stand saves the living entity the game builds: every field in the
/// game's order (health, hurt time, brain…), the given ones read with the game's conversions
/// (health within the maximum, motion beyond 10 read as 0, angles wrapped, default pose parts left
/// out), a field the game does not read dropped.
#[test]
fn summoned_stand_carries_living_fields() {
    let Some((mut p, _dir)) = probe("l9-summon-stand") else { return };
    let plain = summon(&mut p, "minecraft:armor_stand", [112.5, -59.0, 110.5], "{}");
    let e = nbt_of(&p, plain);
    let want: Vec<&str> = vec![
        "id", "Pos", "Motion", "Rotation", "fall_distance", "Fire", "Air", "OnGround", "Invulnerable", "PortalCooldown",
        "UUID", "Health", "HurtTime", "DeathTime", "AbsorptionAmount", "current_impulse_context_reset_grace_time",
        "attributes", "FallFlying", "Brain", "Invisible", "Small", "ShowArms", "DisabledSlots", "NoBasePlate", "Pose",
    ];
    assert_eq!(keys(&e), want, "fields of a summoned stand");
    assert_eq!(e.get("Health"), Some(&Nbt::Float(20.0)));
    assert_eq!(e.get("HurtTime"), Some(&Nbt::Short(0)));
    assert_eq!(e.get("Air"), Some(&Nbt::Short(300)));
    assert_eq!(e.get("Brain"), Some(&Nbt::Compound(vec![("memories".into(), Nbt::Compound(Vec::new()))])));
    let given = "{Foo:1b,Health:50,Small:1,Motion:[11d,0.5d,0d],Rotation:[370f,100f],Tags:[\"t\"],\
                 Pose:{Head:[0f,0f,0f],Body:[5f,0f,0f]},attributes:[{id:\"minecraft:max_health\",base:40d},\
                 {id:\"minecraft:attack_damage\",base:3d}]}";
    let s = summon(&mut p, "minecraft:armor_stand", [114.5, -59.0, 110.5], given);
    let e = nbt_of(&p, s);
    assert!(e.get("Foo").is_none(), "a field the game does not read was kept");
    assert_eq!(e.get("Health"), Some(&Nbt::Float(40.0)), "health clamped to the saved maximum");
    assert_eq!(e.get("Small"), Some(&Nbt::Byte(1)), "integer read as a flag");
    assert_eq!(nbtio::f64s::<3>(e.get("Motion")), Some([0.0, 0.5, 0.0]), "motion beyond 10 read as 0");
    assert_eq!(nbtio::f32s::<2>(e.get("Rotation")), Some([10.0, 90.0]), "angles wrapped");
    assert_eq!(keys(e.get("Pose").expect("pose")), vec!["Body".to_owned()], "default pose part written");
    let attrs = nbtio::list(e.get("attributes"));
    assert_eq!(attrs.len(), 1, "an attribute the stand does not have was kept: {attrs:?}");
    assert!(
        keys(&e).iter().position(|k| k == "Tags") < keys(&e).iter().position(|k| k == "Health"),
        "tags among the entity fields, before the living ones"
    );
}

/// `/summon` of a mannequin: living fields, then its own (profile re-encoded, hidden parts in the
/// game's order, default hand, pose and description).
#[test]
fn summoned_mannequin_carries_living_fields() {
    let Some((mut p, _dir)) = probe("l9-summon-mannequin") else { return };
    let m = summon(&mut p, "minecraft:mannequin", [112.5, -59.0, 110.5], "{profile:\"Notch\",hidden_layers:[\"hat\",\"cape\"]}");
    let e = nbt_of(&p, m);
    let k = keys(&e);
    let tail: Vec<&str> = k.iter().rev().take(5).rev().map(String::as_str).collect();
    assert_eq!(tail, ["profile", "hidden_layers", "main_hand", "pose", "immovable"], "mannequin fields: {k:?}");
    assert!(k.contains(&"Health".to_owned()) && k.contains(&"Brain".to_owned()), "living fields missing: {k:?}");
    assert_eq!(e.get("profile"), Some(&Nbt::Compound(vec![("name".into(), Nbt::String("Notch".into()))])));
    assert_eq!(
        e.get("hidden_layers"),
        Some(&Nbt::List(vec![Nbt::String("cape".into()), Nbt::String("hat".into())])),
        "hidden parts in the game's order"
    );
    assert_eq!(e.get("main_hand"), Some(&Nbt::String("right".into())));
    assert_eq!(e.get("pose"), Some(&Nbt::String("standing".into())));
}
