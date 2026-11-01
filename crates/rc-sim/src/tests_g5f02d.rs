//! G5-F02d: player thorns, wear of its armor, protections and damage that ignores
//! invulnerability; on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`). Absent: tests
//! skipped.
#![cfg(test)]

use crate::blocks::probe::Probe;
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;
use crate::player::Stack;
use rc_proto::nbt::Nbt;

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
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn st(id: &str) -> Option<Stack> {
    Some(Stack { id: id.to_owned(), count: 1, components: None })
}

fn enchanted(id: &str, ench: &str, lvl: i32) -> Option<Stack> {
    let mut s = Stack { id: id.into(), count: 1, components: None };
    crate::items::set_component(&mut s, "minecraft:enchantments", Nbt::Compound(vec![(ench.into(), Nbt::Int(lvl))]));
    Some(s)
}

fn wear(p: &Probe, slot: usize) -> i64 {
    p.s.slots[slot]
        .as_ref()
        .and_then(|s| crate::items::component(s, "minecraft:damage"))
        .and_then(Nbt::as_i64)
        .unwrap_or(0)
}

/// Protection IV on the feet: the void (`bypasses_invulnerability`) goes through whole; ordinary
/// damage without armor (`generic`) stays reduced by 16%.
#[test]
fn protection_skips_bypasses_invulnerability() {
    let Some((mut p, dir)) = probe("g5f02d-prot") else { return };
    p.s.slots[8] = enchanted("minecraft:leather_boots", "minecraft:protection", 4);
    assert!(p.s.hurt("minecraft:out_of_world", 4.0));
    assert_eq!(p.s.life.vit.health, 16.0, "protection applied to the void");
    p.s.life.vit.cooldown = 0;
    assert!(p.s.hurt("minecraft:generic", 4.0));
    assert!((p.s.life.vit.health - (16.0 - 4.0 * 0.84)).abs() < 1e-5, "protection lost: {}", p.s.life.vit.health);
    done(p, dir);
}

/// Elytra on the chest: never worn down by a hit; iron boots are (8 damage: 2 points).
/// Netherite chestplate: not worn down by fire; iron chestplate: worn down.
#[test]
fn armor_wear_follows_equippable_and_resistance() {
    let Some((mut p, dir)) = probe("g5f02d-wear") else { return };
    p.s.slots[6] = st("minecraft:elytra");
    p.s.slots[8] = st("minecraft:iron_boots");
    assert!(p.s.hurt("minecraft:mob_attack", 8.0));
    assert_eq!(wear(&p, 6), 0, "elytra worn down by a hit");
    assert_eq!(wear(&p, 8), 2, "boots not worn down");
    p.s.slots[6] = st("minecraft:netherite_chestplate");
    p.s.life.vit.cooldown = 0;
    assert!(p.s.hurt("minecraft:in_fire", 1.0));
    assert_eq!(wear(&p, 6), 0, "netherite worn down by fire");
    p.s.slots[6] = st("minecraft:iron_chestplate");
    p.s.life.vit.cooldown = 0;
    assert!(p.s.hurt("minecraft:in_fire", 1.0));
    assert_eq!(wear(&p, 6), 1, "iron chestplate not worn down by fire");
    done(p, dir);
}

/// Zero hit: no wear; two pieces broken by the same hit: feet (52) before head (49).
#[test]
fn armor_wear_zero_hit_and_order() {
    let Some((mut p, dir)) = probe("g5f02d-order") else { return };
    p.s.slots[8] = st("minecraft:iron_boots");
    p.s.hurt("minecraft:mob_attack", 0.0);
    assert_eq!(wear(&p, 8), 0, "zero hit that wears down the armor");
    for (slot, id) in [(5, "minecraft:leather_helmet"), (8, "minecraft:leather_boots")] {
        let mut s = Stack { id: id.into(), count: 1, components: None };
        let max = p.s.life.items.max_damage(&s);
        crate::items::set_component(&mut s, "minecraft:damage", Nbt::Int(max - 1));
        p.s.slots[slot] = Some(s);
    }
    p.s.life.vit.cooldown = 0;
    p.take_events();
    assert!(p.s.hurt("minecraft:mob_attack", 4.0));
    let breaks: Vec<i32> = p
        .take_events()
        .iter()
        .filter_map(|e| match e {
            rc_client::Event::Entity(rc_client::EntityEvent::Event { event, .. }) => Some(i32::from(*event)),
            _ => None,
        })
        .filter(|e| (49..=52).contains(e))
        .collect();
    assert_eq!(breaks, vec![52, 49], "break order");
    done(p, dir);
}

/// Thorns (level 10: chance > 1): the zombie that hits takes 1 to 5 damage, the chestplate 2
/// of wear; without thorns, nothing; a hit that does not go through an attack (`/damage`, explosion)
/// triggers nothing.
#[test]
fn thorns_hurt_melee_attacker() {
    let Some((mut p, dir)) = probe("g5f02d-thorns") else { return };
    p.s.slots[6] = enchanted("minecraft:iron_chestplate", "minecraft:thorns", 10);
    let mut z = p.s.new_mob(Kind::Zombie, [110.5, -59.0, 111.5], 180.0);
    let hp = z.health;
    assert!(p.s.mob_attack(&mut z, Ent::Player), "zombie hit refused");
    let lost = hp - z.health;
    assert!((1.0..5.0).contains(&lost), "thorns: {lost} damage");
    // Wear from the hit (⌊d/4⌋ ≥ 1) then 2 from the thorns.
    let w = wear(&p, 6);
    assert!(w >= 3, "thorns wear missing: {w}");
    // Direct hit without an attack: no thorns.
    let mut z2 = p.s.new_mob(Kind::Zombie, [110.5, -59.0, 111.5], 180.0);
    let hp2 = z2.health;
    p.s.life.vit.cooldown = 0;
    assert!(p.s.player_hurt_by_mob(&z2, "minecraft:mob_attack", 2.0, None));
    assert_eq!(z2.health, hp2);
    // Without thorns: the attacker is not hurt.
    p.s.slots[6] = st("minecraft:iron_chestplate");
    p.s.life.vit.cooldown = 0;
    assert!(p.s.mob_attack(&mut z2, Ent::Player));
    assert_eq!(z2.health, hp2, "thorns without enchantment");
    done(p, dir);
}

/// Dragon bite (dragon state taken out, like during its tick) on a player with thorns: the
/// dragon loses HP (body hit: d/4 + min(d, 1), i.e. 1.25 to 2.25), the chestplate wears down.
#[test]
fn thorns_hurt_dragon_during_its_tick() {
    use crate::mobs::end::phases::Phase;
    let Some((mut p, dir)) = probe("g5f02d-dragon") else { return };
    p.s.slots[6] = enchanted("minecraft:diamond_chestplate", "minecraft:thorns", 10);
    let mut m = p.s.new_mob(Kind::EnderDragon, [110.5, -50.0, 110.5], 0.0);
    m.end.dragon.as_mut().expect("dragon").set_phase(Phase::HoldingPattern);
    let hp = m.health;
    let mut d = m.end.dragon.take().expect("dragon");
    let area = p.s.player_aabb();
    p.s.dragon_bite(&mut m, &mut d, area);
    m.end.dragon = Some(d);
    assert!(p.s.life.vit.health < 20.0, "bite without effect");
    let lost = hp - m.health;
    assert!((1.25..=2.25).contains(&lost), "thorns on the dragon: {lost} HP lost");
    assert!(wear(&p, 6) >= 2, "thorns wear missing");
    done(p, dir);
}

/// Melee hit of a behaviour (brain taken out, like during the mob's tick) on a player with
/// thorns: the damage reaction sees the brain. Piglin: admiring disabled (400 ticks); adult
/// hoglin: attack target set again for 200 ticks.
#[test]
fn thorns_reaction_sees_detached_brain() {
    use crate::mobs::brainy::brain::Visible;
    use crate::mobs::piglins::brain::PB;
    let Some((mut p, dir)) = probe("g5f02d-brain") else { return };
    p.s.slots[6] = enchanted("minecraft:diamond_chestplate", "minecraft:thorns", 10);
    for k in [Kind::Piglin, Kind::Hoglin] {
        let mut m = p.s.new_mob(k, [110.5, -59.0, 111.5], 180.0);
        let mut b = m.bm.brain.take().expect("brain");
        b.mem.attack.set_exp(Ent::Player, 30);
        b.mem.visible.set(Visible { list: vec![Ent::Player], cache: vec![Some(true)] });
        let mut pb = Box::new(PB::Melee { cd: 20, adult: None });
        p.s.life.vit.cooldown = 0;
        p.s.life.vit.health = 20.0;
        let hp = m.health;
        let t = p.s.time;
        assert!(p.s.pig_trigger(&mut m, &mut b, &mut pb, t), "{k:?}: no hit");
        assert!(p.s.life.vit.health < 20.0, "{k:?}: player not hit");
        assert!(m.health < hp, "{k:?}: thorns missing");
        match k {
            Kind::Piglin => assert!(b.mem.pig.admiring_disabled.has(), "piglin: thorns reaction lost"),
            _ => assert_eq!(b.mem.attack.ttl, 200, "hoglin: thorns reaction lost"),
        }
    }
    done(p, dir);
}

/// Same cause, other brains. Warden (common brain melee): +100 anger against the player, and
/// the sonic boom cooldown (40) set by its hit. Nautilus (charge): angry.
#[test]
fn thorns_reaction_other_brains() {
    use crate::mobs::brainy::brain::{Bk, Visible};
    let Some((mut p, dir)) = probe("g5f02d-brain2") else { return };
    p.s.slots[6] = enchanted("minecraft:diamond_chestplate", "minecraft:thorns", 10);
    let mut w = p.s.new_mob(Kind::Warden, [110.5, -59.0, 111.5], 180.0);
    w.attrs.attack.base = 2.0;
    let mut b = w.bm.brain.take().expect("brain");
    b.mem.attack.set(Ent::Player);
    b.mem.visible.set(Visible { list: vec![Ent::Player], cache: vec![Some(true)] });
    let hp = w.health;
    let t = p.s.time;
    assert!(p.s.bz_trigger(&mut w, &mut b, &mut Bk::Melee { cd: 18 }, t), "warden: no hit");
    assert!(w.health < hp, "warden: thorns missing");
    let anger = w.rec.anger.suspects.iter().find(|s| s.0 == Ent::Player).map_or(0, |s| s.2);
    assert_eq!(anger, 100, "warden: thorns anger lost");
    assert!(b.mem.x.sonic_cooldown.has(), "warden: sonic boom cooldown not set by the hit");
    p.s.life.vit.cooldown = 0;
    p.s.life.vit.health = 20.0;
    let mut n = p.s.new_mob(Kind::Nautilus, [110.5, -59.0, 110.5], 0.0);
    let mut nb = n.sea.brain.take().expect("nautilus brain");
    let hp = n.health;
    p.s.charge_hit(&mut n, &mut nb);
    assert!(n.health < hp, "nautilus: thorns missing");
    assert_eq!(nb.angry.map(|a| a.0), Some(p.s.uuid), "nautilus: thorns anger lost");
    done(p, dir);
}

/// Unbreaking III, formula chosen by the item: a chestplate held in hand keeps the armor one
/// (each point avoided with 0.3), elytra worn on the chest take the tool one (0.75).
/// 200 points each: about 140 and 50 of wear.
#[test]
fn unbreaking_formula_follows_item() {
    let Some((mut p, dir)) = probe("g5f02d-unbreaking") else { return };
    let both = |id: &str| {
        let mut s = Stack { id: id.into(), count: 1, components: None };
        crate::items::set_component(
            &mut s,
            "minecraft:enchantments",
            Nbt::Compound(vec![("minecraft:unbreaking".into(), Nbt::Int(3)), ("minecraft:thorns".into(), Nbt::Int(1))]),
        );
        Some(s)
    };
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = both("minecraft:netherite_chestplate");
    p.s.slots[6] = both("minecraft:elytra");
    for _ in 0..200 {
        p.s.hurt_item(held, 1);
        p.s.hurt_item(6, 1);
    }
    let (armor, elytra) = (wear(&p, held), wear(&p, 6));
    assert!((110..=170).contains(&armor), "chestplate in hand: {armor} (tool formula?)");
    assert!((25..=80).contains(&elytra), "elytra: {elytra} (armor formula?)");
    done(p, dir);
}

/// Thorns on the shooter: the arrow of a skeleton still in the list hurts it.
#[test]
fn thorns_hurt_shooter_in_list() {
    let Some((mut p, dir)) = probe("g5f02d-shooter") else { return };
    p.s.slots[5] = enchanted("minecraft:iron_helmet", "minecraft:thorns", 10);
    let sk = p.s.new_mob(Kind::Skeleton, [110.5, -59.0, 118.5], 180.0);
    let (id, hp) = (sk.id, sk.health);
    p.s.add_mob(sk);
    p.s.player_thorns_by(Some(Ent::Mob(id)));
    let after = p.s.mob_ref(id).map(|m| m.health).expect("skeleton");
    assert!((1.0..5.0).contains(&(hp - after)), "shooter not hurt: {}", hp - after);
    assert_eq!(wear(&p, 5), 2);
    done(p, dir);
}
