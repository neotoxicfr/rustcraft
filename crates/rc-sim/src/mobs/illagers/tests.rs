//! Tests of family F07 (info sheets, banner, raids, potions).

use super::super::species::{Kind, dims};
use super::*;

#[test]
fn info_sheets_match_the_game() {
    for k in [Kind::Pillager, Kind::Vindicator, Kind::Evoker, Kind::Illusioner] {
        let (w, h, e) = dims(k, false);
        assert_eq!((w, h), (0.6, 1.95));
        assert!((e - 1.6575).abs() < 1e-6, "{k:?} {e}");
        assert!(is_illager(k) && is_raider(k));
    }
    assert_eq!(Kind::Pillager.info().health, 24.0);
    assert_eq!(Kind::Pillager.info().speed, f64::from(0.35f32));
    assert_eq!(Kind::Vindicator.info().follow, 12.0);
    assert_eq!(Kind::Evoker.info().xp, 10);
    assert_eq!(Kind::Illusioner.info().health, 32.0);
    let v = Kind::Vex.info();
    assert_eq!((v.width, v.height, v.eye, v.health, v.attack, v.xp), (0.4, 0.8, 0.518_75, 14.0, 4.0, 3));
    assert!(v.fire_immune);
    let r = Kind::Ravager.info();
    assert_eq!((r.width, r.height, r.health, r.attack, r.step, r.xp), (1.95, 2.2, 100.0, 12.0, 1.0, 20));
    let w = Kind::Witch.info();
    assert_eq!((w.eye, w.health, w.speed), (1.62, 26.0, 0.25));
    assert!(is_raider(Kind::Witch) && !is_illager(Kind::Witch) && !is_raider(Kind::Vex));
    for k in [Kind::Pillager, Kind::Vex, Kind::Ravager, Kind::Witch] {
        assert_eq!(Kind::from_name(k.info().name), Some(k));
        assert!(k.is_monster());
    }
    assert!(!can_be_leader(Kind::Ravager) && !can_be_leader(Kind::Witch) && can_be_leader(Kind::Evoker));
}

#[test]
fn ominous_banner_properties() {
    let b = ominous_banner();
    assert!(is_ominous_banner(&b));
    let mut other = b.clone();
    other.count = 2;
    assert!(!is_ominous_banner(&other));
    let plain = crate::player::Stack { id: "minecraft:white_banner".into(), count: 1, components: None };
    assert!(!is_ominous_banner(&plain));
    let Some(Nbt::List(p)) = b.components.as_ref().and_then(|c| c.get("minecraft:banner_patterns")).cloned() else {
        panic!("patterns")
    };
    assert_eq!(p.len(), 8);
}

#[test]
fn raid_wave_counts() {
    assert_eq!(raids::num_groups(1), 3);
    assert_eq!(raids::num_groups(2), 5);
    assert_eq!(raids::num_groups(3), 7);
    let mut r = raids::test_raid([0, 64, 0], 2);
    assert_eq!((r.num_groups, r.cooldown, r.status), (5, 300, 0));
    // UUID version 4, IETF variant.
    assert_eq!((r.bar >> 76) & 0xF, 4);
    assert_eq!(r.bar >> 62 & 3, 2);
    r.omen = 3;
    assert_eq!(r.enchant_odds(), 0.25);
    r.omen = 1;
    assert_eq!(r.enchant_odds(), 0.0);
}

#[test]
fn potion_colors() {
    // Single color: the effect's color, opaque (common projectile color).
    let color = |p: &str| crate::projectiles::hits::potion_color(&witch::potion_stack("minecraft:splash_potion", p));
    let c = color("minecraft:poison");
    assert_eq!(c as u32 >> 24, 0xFF);
    assert_eq!(color("minecraft:water"), -13_083_194);
}

#[test]
fn plain_text_of_name() {
    assert_eq!(plain_name(&Nbt::String("Johnny".into())), "Johnny");
    let c = Nbt::Compound(vec![
        ("text".into(), Nbt::String("Joh".into())),
        ("extra".into(), Nbt::List(vec![Nbt::String("nny".into())])),
    ]);
    assert_eq!(plain_name(&c), "Johnny");
}

#[test]
fn block_center_checks() {
    assert!(goals::closer_to_center([0, 0, 0], [0.5, 0.5, 9.9], 10.0));
    assert!(!goals::closer_to_center([0, 0, 0], [0.5, 0.5, 10.6], 10.0));
    assert!(raids::test_dist([0.0; 3], [3.0, 4.0, 0.0]) == 25.0);
}

/// The drinking witch only raises her own data (17), never the used-item flag (8).
#[test]
fn drinking_witch_sets_no_used_item_flag() {
    let mut m = super::super::mob::Mob::new(Kind::Witch, 1, 1, [0.0; 3], 0.0, super::super::jrand::JRand::new(1));
    m.define_data();
    m.ill.drinking = true;
    m.use_ticks = 0;
    sync_data(&mut m);
    assert_eq!(m.get(8), Some(&Meta::Byte(0)));
    assert_eq!(m.get(17), Some(&Meta::Bool(true)));
    let mut p = super::super::mob::Mob::new(Kind::Pillager, 2, 2, [0.0; 3], 0.0, super::super::jrand::JRand::new(1));
    p.define_data();
    p.use_ticks = 0;
    sync_data(&mut p);
    assert_eq!(p.get(8), Some(&Meta::Byte(1)));
}
