//! Knockback direction preserved on diverted damage paths (copy of the test world,
//! `RUSTCRAFT_SOLO_WORLD`, zone 200..231): dragon hit by a projectile, instant damage cloud
//! on a mob or on the player.

use super::super::mob::Ent;
use super::super::species::Kind;
use super::phases::Phase;
use crate::blocks::probe::Probe;
use crate::player::Stack;

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -50 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    p.s.game_mode = 0;
    p.s.view = 8;
    p.s.loaded = true;
    Some((p, dir))
}

fn harming_cloud(p: &mut Probe, pos: [f64; 3], owner: Option<u128>) {
    let mut s = Stack { id: "minecraft:lingering_potion".into(), count: 1, components: None };
    crate::items::set_component(
        &mut s,
        "minecraft:potion_contents",
        rc_proto::nbt::Nbt::Compound(vec![("potion".into(), rc_proto::nbt::Nbt::String("minecraft:harming".into()))]),
    );
    let mut c = crate::projectiles::cloud::Cloud::new(9_000_001, pos);
    c.potion = Some(s);
    c.owner = owner;
    c.wait_time = 0;
    p.s.cloud_add(c);
    for _ in 0..6 {
        p.s.clouds_tick();
    }
}

/// Player arrow on the flying dragon: the knockback follows exactly the direction given by the
/// projectile, without going back through a position (the position round trip loses bits).
#[test]
fn dragon_keeps_projectile_knockback_dir() {
    let Some((mut p, dir)) = probe("kbdragon") else { return };
    let mut m = p.s.new_mob(Kind::EnderDragon, [216.5, -40.0, 216.5], 0.0);
    m.end.dragon.as_mut().expect("dragon").set_phase(Phase::HoldingPattern);
    let (id, pos, v0, res) = (m.id, m.pos, m.vel, m.attrs.knockback_resistance.value());
    p.s.add_mob(m);
    let kb = (0.1, 0.3);
    // The round trip through a position is not exact here: the test distinguishes the two paths.
    assert_ne!((pos[0] + kb.0) - pos[0], kb.0);
    assert!(p.s.proj_damage(Ent::Mob(id), "minecraft:arrow", 10.0, Some(Ent::Player), kb));
    let m = p.s.mob_ref(id).expect("dragon");
    let power = 0.4 * (1.0 - res);
    let l = (kb.0 * kb.0 + kb.1 * kb.1).sqrt();
    assert_eq!(m.vel[0], v0[0] / 2.0 - kb.0 / l * power);
    assert_eq!(m.vel[2], v0[2] / 2.0 - kb.1 / l * power);
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ownerless instant damage cloud on a pig: indirect magic whose source is the cloud, knockback
/// away from the cloud (and no longer in a randomly drawn direction).
#[test]
fn cloud_harm_knocks_mob_away_from_cloud() {
    let Some((mut p, dir)) = probe("kbcloudmob") else { return };
    let m = p.s.new_mob(Kind::Pig, [216.5, -59.0, 212.5], 0.0);
    let (id, health) = (m.id, m.health);
    p.s.add_mob(m);
    harming_cloud(&mut p, [215.5, -59.0, 212.5], None);
    let m = p.s.mob_ref(id).expect("pig");
    assert!(m.health < health, "instant damage missing");
    assert_eq!((m.vel[0], m.vel[2]), (0.4, 0.0));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Same cloud on the player: knockback from the cloud (nothing before, without an owner mob).
#[test]
fn cloud_harm_knocks_player_away_from_cloud() {
    let Some((mut p, dir)) = probe("kbcloudplayer") else { return };
    let health = p.s.life.vit.health;
    p.s.mobs.player_vel = [0.0; 3];
    let pos = p.s.pos;
    harming_cloud(&mut p, [pos[0] + 1.0, pos[1], pos[2]], None);
    assert!(p.s.life.vit.health < health, "instant damage missing");
    let v = p.s.mobs.player_vel;
    assert_eq!((v[0], v[2]), (-0.4, 0.0));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}
