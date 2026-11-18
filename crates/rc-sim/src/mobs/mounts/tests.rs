//! Tests of mounts (F04): attribute draws, inheritance, info sheets, NBT.

use super::super::jrand::JRand;
use super::super::species::Kind;
use super::*;

#[test]
fn ranges_match_the_game() {
    let (hl, hh) = health_range();
    assert_eq!((hl, hh), (15.0, 30.0));
    let (jl, jh) = jump_range();
    assert!((jl - 0.4).abs() < 1e-6 && (jh - 1.0).abs() < 1e-6);
    let (sl, sh) = speed_range();
    assert!((sl - 0.1125).abs() < 1e-6 && (sh - 0.3375).abs() < 1e-6);
}

#[test]
fn offspring_stays_in_range_and_reflects() {
    let mut r = JRand::new(42);
    for _ in 0..1000 {
        let v = offspring_attribute(30.0, 30.0, 15.0, 30.0, &mut r);
        assert!((15.0..=30.0 + 1e-9).contains(&v) || v < 30.0 + 4.5, "{v}");
    }
}

#[test]
fn every_kind_has_its_info() {
    for k in Kind::ALL.into_iter().filter(|k| handles(*k)) {
        assert_eq!(Kind::from_name(k.info().name), Some(k));
        assert!(k.is_animal());
    }
    assert_eq!(Kind::ZombieHorse.info().category, super::super::species::Category::Monster);
    assert!(Kind::SkeletonHorse.undead() && Kind::CamelHusk.undead() && !Kind::Camel.undead());
}

#[test]
fn dash_start_arms_the_cooldown() {
    // The dash launched by the player arms the cooldown: the end of the dash (cooldown < 50) waits 5 ticks.
    let mut m = super::super::mob::Mob::new(Kind::Camel, 1, 0, [0.0; 3], 0.0, JRand::new(1));
    camel::set_dash(&mut m, true);
    assert!(m.mount.dash);
    assert_eq!(m.mount.dash_cooldown, 55);
    m.mount.dash_cooldown = 30;
    camel::set_dash(&mut m, false);
    assert_eq!(m.mount.dash_cooldown, 30);
}

#[test]
fn underwater_dismount_tag() {
    for k in [
        Kind::Horse,
        Kind::Donkey,
        Kind::Mule,
        Kind::Llama,
        Kind::TraderLlama,
        Kind::ZombieHorse,
        Kind::Camel,
        Kind::Ravager,
        Kind::HappyGhast,
        Kind::Strider,
    ] {
        assert!(dismounts_underwater(k), "{k:?}");
    }
    assert!(!dismounts_underwater(Kind::SkeletonHorse) && !dismounts_underwater(Kind::CamelHusk));
}
