//! Tests of aquatic fauna (F09): info sheets, variants, draws, schools, movements.

use super::super::jrand::JRand;
use super::super::species::{Category, Kind};
use super::fish;

#[test]
fn info_sheets_and_categories() {
    assert_eq!(Kind::Cod.info().category, Category::WaterAmbient);
    assert_eq!(Kind::GlowSquid.info().category, Category::UndergroundWaterCreature);
    assert_eq!(Kind::Dolphin.info().category, Category::WaterCreature);
    assert_eq!(Kind::ZombieNautilus.info().category, Category::Monster);
    assert_eq!(Category::WaterAmbient.despawn_distance(), 64);
    assert_eq!(Category::WaterAmbient.max(), 20);
    assert_eq!(Category::WaterCreature.max(), 5);
    assert_eq!(Kind::from_name("minecraft:tropical_fish"), Some(Kind::TropicalFish));
    assert!(Kind::Squid.ageable() && Kind::Nautilus.ageable() && !Kind::ZombieNautilus.ageable());
    assert!(!Kind::Cod.ageable());
    assert!(Kind::ZombieNautilus.undead());
}

#[test]
fn tropical_fish_variants() {
    // Striped orange / gray: pattern 257, base 1, pattern 7.
    assert_eq!(fish::COMMON[0], 257 | (1 << 16) | (7 << 24));
    // Unknown pattern → kob; color out of bounds → white.
    assert_eq!(fish::normalize(0x7777 | (20 << 16)), 0);
    assert_eq!(fish::normalize(fish::COMMON[5]), fish::COMMON[5]);
}

#[test]
fn salmon_sizes() {
    assert_eq!(fish::salmon_scale(0), 0.5);
    assert_eq!(fish::salmon_scale(7), 1.5);
    // Same sequence as the weighted draw (one draw out of 95).
    let mut a = JRand::new(42);
    let mut b = JRand::new(42);
    let v = fish::random_salmon_size(&mut a);
    let n = b.next_int_bound(95);
    assert_eq!(v, if n < 30 { 0 } else if n < 80 { 1 } else { 2 });
}

/// Diagnostics of the world test prerequisites (displayed, never failing).
#[test]
fn world_test_prerequisites() {
    eprintln!("F09: test world {:?}, data {}", crate::tests::test_world(), rc_testdata::version_data().is_some());
}

/// Dolphin jump: the controlled corridor (water, two blocks of air above) follows its yaw, not its
/// velocity. In a one-cell channel bordered by grass, it therefore does not jump when it looks at the
/// bank, even if its velocity is mostly vertical; in open water in front of it, it jumps
/// forward even when pushed sideways.
#[test]
fn dolphin_jump_follows_the_gaze() {
    use super::goals::AGoal;
    use crate::blocks::probe::Probe;
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return;
    };
    let dir = super::super::tests::prep(&world, "dolphin-jump", 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(concat!(
        "fill 200 -60 200 231 -60 231 minecraft:stone\n",
        "fill 200 -59 200 231 -40 231 minecraft:air\n",
        "fill 200 -59 200 231 -59 231 minecraft:grass_block\n",
        // One-cell channel along x (z = 205); long pool along z (x = 220).
        "fill 203 -59 205 228 -59 205 minecraft:water\n",
        "fill 220 -59 208 220 -59 230 minecraft:water\n",
        "player 216.5 -58 214.5 0 0\n",
    ));
    let jump = || AGoal::Jump { interval: 1, breached: false };

    // Look toward the bank (south, +z), velocity mostly vertical: no jump.
    let mut m = p.s.new_mob(Kind::Dolphin, [215.5, -59.0, 205.5], 0.0);
    m.vel = [0.0, -0.005, 0.0];
    assert!(!p.s.dolphin_can_use(&mut jump(), &mut m), "jump out of the channel, toward the grass");

    // Look along the pool (south), pushed toward the east (grass): jump toward the south.
    let mut m = p.s.new_mob(Kind::Dolphin, [220.5, -59.0, 210.5], 0.0);
    m.vel = [0.1, 0.0, 0.0];
    let mut g = jump();
    assert!(p.s.dolphin_can_use(&mut g, &mut m), "open water in front of the dolphin");
    p.s.dolphin_start(&mut g, &mut m);
    assert_eq!(m.vel, [0.1, 0.7, 0.6], "momentum in the gaze direction");

    // Stranded on the ground in flowing water: it looks for water (tag `dolphin_tries_to_find` =
    // source only) and targets the first source of the box, not the flowing water met before.
    p.run(concat!(
        "setblock 208 -59 218 minecraft:water[level=1]\n",
        "setblock 210 -59 220 minecraft:water[level=1]\n",
        "setblock 212 -59 222 minecraft:water\n",
    ));
    let mut m = p.s.new_mob(Kind::Dolphin, [210.5, -59.0, 220.5], 0.0);
    m.on_ground = true;
    let mut g = AGoal::FindLiquid;
    assert!(p.s.dolphin_can_use(&mut g, &mut m), "flowing water does not count as a refuge");
    p.s.dolphin_start(&mut g, &mut m);
    assert_eq!(m.move_ctl.wanted, [212.0, -59.0, 222.0], "only source water is targeted");

    // Same pool, look toward the west (grass): no jump, whatever the velocity.
    let mut m = p.s.new_mob(Kind::Dolphin, [220.5, -59.0, 210.5], 90.0);
    m.vel = [0.0, 0.0, 0.3];
    assert!(!p.s.dolphin_can_use(&mut jump(), &mut m), "corridor taken along the velocity");
}
