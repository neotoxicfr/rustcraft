//! Falling blocks and primed TNT in singleplayer (test world `RUSTCRAFT_SOLO_WORLD`).

use super::*;
use crate::blocks::probe::{Probe, state_string};

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    // Loaded zone (test world near the origin): stone floor placed.
    assert_eq!(name_at(&p, [100, -60, 100]), "minecraft:stone", "test zone missing");
    Some((p, dir))
}

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn name_at(p: &Probe, at: P) -> String {
    state_string(&p.s, p.s.get(at))
}

fn items_named(p: &Probe, id: &str) -> i32 {
    p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
}

/// Sand dropped on a torch: the cell is occupied, the sand falls as an item instead of
/// disappearing; the torch stays.
#[test]
fn sand_on_torch_drops_as_item() {
    let Some((mut p, dir)) = probe("w4b-torch") else { return };
    let mut s = String::from(BASE);
    s.push_str("setblock 105 -59 105 minecraft:torch\nsetblock 105 -50 105 minecraft:sand\ntick 40\n");
    p.run(&s);
    assert_eq!(name_at(&p, [105, -59, 105]), "minecraft:torch");
    assert_eq!(name_at(&p, [105, -50, 105]), "minecraft:air");
    assert_eq!(items_named(&p, "minecraft:sand"), 1, "sand dropped as an item");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// Primed TNT: the crater drops its loot (decay disabled for TNT), a neighbouring TNT
/// is primed with a short fuse and a primed TNT nearby is blown away.
#[test]
fn primed_tnt_explosion_drops_chains_and_pushes() {
    let Some((mut p, dir)) = probe("w4b-tnt") else { return };
    let mut s = String::from(BASE);
    s.push_str("fill 108 -59 108 113 -59 112 minecraft:dirt\nsetblock 110 -58 112 minecraft:tnt\n");
    p.run(&s);
    p.s.tnt_prime([110, -58, 110]);
    p.s.tnt_prime([113, -58, 110]);
    let far = p.s.bs.entities.list.last().map(|e| e.id).expect("TNT");
    // The second keeps a long fuse: we lengthen it so that it survives the first.
    if let Some(e) = p.s.bs.entities.list.last_mut() {
        e.kind = EntKind::Tnt(200);
    }
    p.run("tick 81\n");
    assert_eq!(name_at(&p, [110, -59, 110]), "minecraft:air", "crater");
    assert!(items_named(&p, "minecraft:dirt") > 0, "crater loot");
    // Neighbouring TNT blown away: replaced by a primed TNT with a short fuse (10 to 29 ticks).
    assert_eq!(name_at(&p, [110, -58, 112]), "minecraft:air");
    let chained: Vec<i32> =
        p.s.bs
            .entities
            .list
            .iter()
            .filter(|e| e.id != far && !e.removed)
            .filter_map(|e| match e.kind {
                EntKind::Tnt(f) => Some(f),
                _ => None,
            })
            .collect();
    assert_eq!(chained.len(), 1, "chained TNT: {chained:?}");
    assert!((9..=29).contains(&chained[0]), "short fuse: {}", chained[0]);
    // The primed TNT 3 blocks away is pushed east (increasing x). It rests on the dirt
    // platform (which extends to x = 113): placed at the edge, the random priming impulse made
    // it fall off by one block every other time, hidden behind the platform (visible part 0, no
    // blast, as in the game). On the platform, it is always in view: x ≥ 114.5 after the
    // blast.
    let pushed = p.s.bs.entities.list.iter().find(|e| e.id == far).expect("distant TNT");
    assert!(pushed.pos[1] > -58.5, "TNT stayed on the platform: y = {}", pushed.pos[1]);
    assert!(pushed.pos[0] > 114.0, "TNT blown away: x = {}", pushed.pos[0]);
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// Without the `tnt_explodes` rule, the primed TNT goes out without destroying anything.
#[test]
fn tnt_explodes_rule_off_keeps_blocks() {
    let Some((mut p, dir)) = probe("w4b-tntoff") else { return };
    let mut s = String::from(BASE);
    s.push_str("fill 108 -59 108 112 -59 112 minecraft:dirt\n");
    p.run(&s);
    crate::nbtio::set(&mut p.s.rules, "minecraft:tnt_explodes", Nbt::Byte(0));
    p.s.tnt_prime([110, -58, 110]);
    p.run("tick 81\n");
    assert_eq!(name_at(&p, [110, -59, 110]), "minecraft:dirt");
    assert_eq!(items_named(&p, "minecraft:dirt"), 0);
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// A blown-up TNT block is primed with a short fuse: it leaves with the spawn, otherwise
/// the client would think 80 ticks remain.
#[test]
fn chained_tnt_sends_its_short_fuse() {
    let Some((mut p, dir)) = probe("w4b-fuse") else { return };
    p.run(BASE);
    let _ = p.take_events();
    p.s.tnt_was_exploded([110, -58, 110]);
    let fuse = p.s.bs.entities.list.last().and_then(|e| if let EntKind::Tnt(f) = e.kind { Some(f) } else { None });
    let sent: Vec<i32> = p
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Entity(EntityEvent::Data { values, .. }) => values.into_iter().find_map(|(i, m)| match m {
                rc_proto_v777::entity::Meta::Int(v) if i == 8 => Some(v),
                _ => None,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "fuse announced to the client");
    assert_eq!(Some(sent[0]), fuse);
    assert!((10..30).contains(&sent[0]));
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// Anvil damaged on landing on an occupied cell: the dropped item is the one from before the
/// fall (26.3). Fall of more than 20 blocks: the wear is certain.
#[test]
fn anvil_blocked_drops_original_item() {
    let Some((mut p, dir)) = probe("w4b-anvil") else { return };
    let mut s = String::from(BASE);
    s.push_str("setblock 105 -59 105 minecraft:torch\nsetblock 105 -25 105 minecraft:anvil\ntick 90\n");
    p.run(&s);
    assert_eq!(name_at(&p, [105, -59, 105]), "minecraft:torch");
    assert_eq!(items_named(&p, "minecraft:anvil"), 1, "item from before the fall");
    assert_eq!(items_named(&p, "minecraft:chipped_anvil") + items_named(&p, "minecraft:damaged_anvil"), 0);
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// Fall distance reset: a movement of one block or more that crosses
/// water or a cobweb; below one block, the fall keeps counting.
#[test]
fn fall_ray_resets_in_water_only_from_one_block() {
    let Some((mut p, dir)) = probe("w4b-reset") else { return };
    p.run(&format!("{BASE}fill 105 -59 105 105 -50 105 minecraft:water\nsetblock 108 -55 108 minecraft:cobweb\n"));
    let at = [105.5, -49.0, 105.5];
    let mut fall = 10.0;
    assert!(!p.s.fall_step(&mut fall, at, [0.0, -1.96, 0.0], false, false));
    assert!((fall - 1.96).abs() < 1.0e-6, "reset then movement accumulated: {fall}");
    let mut fall = 10.0;
    p.s.fall_step(&mut fall, at, [0.0, -0.9, 0.0], false, false);
    assert!((fall - 10.9).abs() < 1.0e-6, "under one block: the fall continues: {fall}");
    // Ray that misses the water: the distance accumulates.
    let mut fall = 10.0;
    p.s.fall_step(&mut fall, [110.5, -45.0, 110.5], [0.0, -1.96, 0.0], false, false);
    assert!((fall - 11.96).abs() < 1.0e-5, "out of the water: {fall}");
    // Cobweb from the tag.
    let mut fall = 10.0;
    p.s.fall_step(&mut fall, [108.5, -50.0, 108.5], [0.0, -5.0, 0.0], false, false);
    assert!((fall - 5.0).abs() < 1.0e-6, "cobweb: {fall}");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// Item, TNT and falling block emit `hit_ground` on landing.
#[test]
fn landing_emits_hit_ground_for_plain_entities() {
    let Some((mut p, dir)) = probe("w4b-hit") else { return };
    p.run(BASE);
    p.s.mobs.rec.vib.log.clear();
    p.s.spawn_item(
        [105.5, -50.0, 105.5],
        [0.0; 3],
        crate::player::Stack { id: "minecraft:dirt".into(), count: 1, components: None },
        0,
    );
    for _ in 0..30 {
        p.run("tick 1\n");
        p.s.drops_tick();
    }
    let n = |p: &Probe| p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == "minecraft:hit_ground").count();
    assert_eq!(n(&p), 1, "item");
    p.s.tnt_prime([110, -50, 110]);
    p.run("tick 30\n");
    assert_eq!(n(&p), 2, "TNT");
    p.run("setblock 115 -50 115 minecraft:sand\ntick 30\n");
    assert_eq!(n(&p), 3, "falling block");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// Concrete powder placed by its fall next to water (neighbouring waterlogged block): it hardens,
/// like landing (water touching a non-full face); without neighbouring water, it stays.
#[test]
fn concrete_powder_landing_beside_water_solidifies() {
    let Some((mut p, dir)) = probe("g5f01-concrete") else { return };
    p.run("setblock 124 -59 120 minecraft:white_concrete_powder\nsetblock 121 -59 120 minecraft:oak_slab[waterlogged=true]\n");
    let air = p.s.air();
    let s = p.s.get([124, -59, 120]);
    // Placed without a shape update: only the landing can harden it.
    p.s.set_block([120, -59, 120], s, flag::CLIENTS | flag::KNOWN_SHAPE);
    assert_eq!(name_at(&p, [120, -59, 120]), "minecraft:white_concrete_powder", "hardened before landing");
    p.s.on_land([120, -59, 120], s, air);
    assert_eq!(name_at(&p, [120, -59, 120]), "minecraft:white_concrete");
    p.s.on_land([124, -59, 120], s, air);
    assert_eq!(name_at(&p, [124, -59, 120]), "minecraft:white_concrete_powder");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}