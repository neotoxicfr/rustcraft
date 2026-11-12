//! G6-F09b: arrows on pressure plates (pass over the traversed blocks), the
//! `projectiles_can_break_blocks` rule, the `impact_projectiles` tag and the adventure mode check
//! before breaking on impact. Fresh flat world of G6-F01: a missing 26.3 cache or client jar makes
//! the test FAIL.

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::g6f01_tests::{W, Y, stack, world};
use super::*;
use crate::blocks::rules::*;
use crate::projectiles::physics::BlockHit;
use crate::projectiles::{PKind, Proj};

fn game_ticks(w: &mut W, n: i32, out: &mut Vec<Event>) {
    for _ in 0..n {
        w.p.s.game_tick();
        w.p.s.tick += 1;
        w.p.s.flush_block_changes();
        out.extend(w.p.take_events());
    }
}

/// The frozen predicate of the impact tag matches the jar's tag for every simulated kind; the
/// fishing bobber is not part of it.
#[test]
fn impact_tag_matches_jar() {
    let w = world("g6f09b-tag");
    let tags = |n: &str| w.p.s.entity_tags_named(n);
    assert!(!tags("minecraft:arrow").is_empty(), "entity type tags missing");
    let mut wrong = Vec::new();
    for k in PKind::ALL {
        let want = tags(k.name()).iter().any(|t| t == "minecraft:impact_projectiles");
        if k.impact() != want {
            wrong.push(format!("{k:?} expected {want}"));
        }
    }
    assert!(wrong.is_empty(), "differences:\n{}", wrong.join("\n"));
    assert!(!tags("minecraft:fishing_bobber").iter().any(|t| t == "minecraft:impact_projectiles"));
}

/// Arrow stuck in a wooden plate and in a light weighted plate: pressed (15 and 1) as long as it
/// stays, released on the tick after its removal; a stone plate ignores it.
#[test]
fn arrows_press_wooden_and_weighted_plates() {
    let mut w = world("g6f09b-plate");
    let (oak, stone, light) = ([4, Y, 4], [8, Y, 4], [12, Y, 4]);
    w.set(oak, "minecraft:oak_pressure_plate");
    w.set(stone, "minecraft:stone_pressure_plate");
    w.set(light, "minecraft:light_weighted_pressure_plate");
    let _ = w.events();
    for x in ["4.5", "8.5", "12.5"] {
        w.cmd(&format!("summon minecraft:arrow {x} {} 4.5 {{Motion:[0.0d,-0.2d,0.0d]}}", f64::from(Y) + 0.3));
    }
    let mut evs = Vec::new();
    game_ticks(&mut w, 3, &mut evs);
    let sig = |w: &W, at: P| w.p.s.plate_signal(w.p.s.get(at));
    assert_eq!(sig(&w, oak), 15, "wooden plate pressed by the arrow");
    assert_eq!(sig(&w, stone), 0, "stone plate: living entities only");
    assert_eq!(sig(&w, light), 1, "light weighted plate: one entity");
    game_ticks(&mut w, 40, &mut evs);
    assert_eq!(sig(&w, oak), 15, "held as long as the arrow stays");
    for a in w.p.s.proj.list.iter_mut() {
        a.removed = true;
    }
    game_ticks(&mut w, 21, &mut evs);
    assert_eq!(sig(&w, oak), 0, "released after the arrow is removed");
    assert_eq!(sig(&w, light), 0);
}

fn proj(w: &W, kind: PKind, item: &str) -> Proj {
    let mut p = Proj::new(kind, 900_950, [4.5, f64::from(Y) + 1.5, 4.5], stack(item, 1).unwrap());
    p.owner = Some(w.p.s.uuid);
    p
}

/// Block placed, hit by `p` from above: true if the block was broken.
fn broken(mut p: Proj, w: &mut W, block: &str) -> bool {
    let at = [4, Y, 4];
    w.set(at, block);
    let _ = w.events();
    let hit = BlockHit { loc: [4.5, f64::from(Y) + 1.0, 4.5], pos: at, face: 1, inside: false, border: false };
    w.p.s.block_projectile_hit(&mut p, hit);
    let gone = w.p.s.is_air(at);
    w.set(at, "minecraft:air");
    gone
}

/// Breaking on impact: impact tag (arrow, firework yes; pearl, potion, hook no), the
/// `projectiles_can_break_blocks` rule, adventure (player without `can_break`, mob shooter).
#[test]
fn impact_break_needs_tag_rule_and_adventure_rights() {
    let mut w = world("g6f09b-break");
    let pot = "minecraft:decorated_pot";
    let chorus_base = [4, Y - 1, 4];
    w.set(chorus_base, "minecraft:end_stone");
    let chorus = "minecraft:chorus_flower[age=0]";

    assert!(broken(proj(&w, PKind::Arrow, "minecraft:arrow"), &mut w, pot), "arrow: pot broken");
    assert!(broken(proj(&w, PKind::Firework, "minecraft:firework_rocket"), &mut w, chorus), "firework: chorus broken");
    assert!(!broken(proj(&w, PKind::EnderPearl, "minecraft:ender_pearl"), &mut w, pot), "pearl outside the tag");
    assert!(!broken(proj(&w, PKind::SplashPotion, "minecraft:splash_potion"), &mut w, pot), "potion outside the tag");
    assert!(!broken(proj(&w, PKind::ShulkerBullet, "minecraft:air"), &mut w, chorus), "bullet outside the tag");
    // Fast trident (> 0.6 block per tick): both speleothems break, a slow trident does not.
    for spike in ["minecraft:pointed_dripstone", "minecraft:sulfur_spike"] {
        let mut t = proj(&w, PKind::Trident, "minecraft:trident");
        t.vel = [0.0, -2.5, 0.0];
        assert!(broken(t, &mut w, spike), "fast trident: {spike} broken");
        let mut t = proj(&w, PKind::Trident, "minecraft:trident");
        t.vel = [0.0, -0.5, 0.0];
        assert!(!broken(t, &mut w, spike), "slow trident: {spike} intact");
    }

    // Hook: a real rod cast straight down onto a pot (snowball projectile reused, but outside
    // the impact tag).
    let at = [4, Y, 4];
    w.set(at, pot);
    let _ = w.events();
    w.p.s.pos = [4.5, f64::from(Y) + 2.0, 3.9];
    (w.p.s.yaw, w.p.s.pitch) = (0.0, 89.0);
    w.p.s.slots[36 + w.p.s.held as usize] = stack("minecraft:fishing_rod", 1);
    assert!(w.p.s.fishing_rod_use(0), "rod cast");
    let mut evs = Vec::new();
    game_ticks(&mut w, 10, &mut evs);
    let h = w.p.s.proj.hook.as_ref().expect("hook still there");
    assert!(h.p.pos[1] < f64::from(Y) + 1.2, "hook resting on the pot: {:?}", h.p.pos);
    assert!(!w.p.s.is_air(at), "hook: pot intact");
    w.p.s.proj.hook = None;
    w.p.s.slots[36 + w.p.s.held as usize] = None;
    w.set(at, "minecraft:air");

    // Rule off: nothing breaks any more.
    w.cmd("gamerule projectiles_can_break_blocks false");
    assert!(!broken(proj(&w, PKind::Arrow, "minecraft:arrow"), &mut w, pot), "rule off: pot intact");
    assert!(!broken(proj(&w, PKind::Arrow, "minecraft:arrow"), &mut w, chorus), "rule off: chorus intact");
    w.cmd("gamerule projectiles_can_break_blocks true");

    // Mob shooter: `can_break` component required (never carried), even with mob_griefing.
    w.cmd(&format!("summon minecraft:zombie 2.5 {Y} 2.5 {{NoAI:1b}}"));
    let zombie = w.p.s.mobs.list.iter().find(|m| !m.removed).map(|m| m.uuid).expect("zombie");
    let mut a = proj(&w, PKind::Arrow, "minecraft:arrow");
    a.owner = Some(zombie);
    assert!(!broken(a, &mut w, pot), "mob's arrow: pot intact");

    // Player in adventure: only with `can_break` for this block.
    w.p.s.game_mode = 2;
    assert!(!broken(proj(&w, PKind::Arrow, "minecraft:arrow"), &mut w, pot), "adventure without can_break");
    let mut a = proj(&w, PKind::Arrow, "minecraft:arrow");
    a.item.components = Some(Nbt::Compound(vec![(
        "minecraft:can_break".into(),
        Nbt::Compound(vec![("blocks".into(), Nbt::String(pot.into()))]),
    )]));
    assert!(broken(a, &mut w, pot), "adventure with can_break: pot broken");
    w.p.s.game_mode = 0;
}
