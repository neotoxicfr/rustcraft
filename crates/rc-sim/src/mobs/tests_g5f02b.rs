//! G5-F02b: entity sound seeds independent of the world's random source.

use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mob sound, an entity-attached sound or a world sound draws nothing from the mobs' world
/// random source (the official game has a generator of its own for sound seeds): spawns and
/// finalizations do not depend on the number of sounds played.
#[test]
fn sounds_draw_nothing_from_world_random() {
    let Some((mut p, dir)) = probe("g5f02b-seed") else { return };
    let m = p.s.new_mob(Kind::Zombie, [210.5, -59.0, 210.5], 0.0);
    let mut before = p.s.mobs.rng;
    p.s.mob_sound(&m, "minecraft:entity.zombie.ambient", 1.0, 1.0);
    p.s.entity_sound(m.id, "minecraft:entity.zombie.ambient", 5, 1.0, 1.0);
    p.s.world_sound("minecraft:entity.zombie.ambient", 5, m.pos, 1.0, 1.0);
    assert_eq!(p.s.mobs.rng.next_long(), before.next_long(), "world random advanced by a sound");
    assert!(!p.take_events().is_empty(), "no sound sent");
    cleanup(p, dir);
}
