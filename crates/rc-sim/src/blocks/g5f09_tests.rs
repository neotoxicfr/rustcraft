//! G5-F09: block rules (big dripleaf, note block, bell, chorus, amethyst,
//! heads, beehive, candle cake, oxidation, campfire, Eye of Ender, biome zoom).
//! Fresh flat world from `g6f01_tests`: the 26.3 cache and the client jar are required (the test
//! fails without them).

use rc_client::Event;
use rc_proto_v777::play::{SoundPacket, SoundRef};

use super::g6f01_tests::{W, Y, stack, world};
use super::*;
use crate::blocks::probe::state_string;
use crate::projectiles::physics::BlockHit;
use crate::projectiles::{PKind, Proj};

fn name(w: &W, at: P) -> String {
    state_string(&w.p.s, w.p.s.get(at))
}

fn sounds<'a>(w: &W, evs: &'a [Event], sound: &str) -> Vec<&'a SoundPacket> {
    let id = w.p.s.sound_id(sound).expect("known sound");
    evs.iter()
        .filter_map(|e| match e {
            Event::Sound(s) if s.sound == SoundRef::Id(id) => Some(s),
            _ => None,
        })
        .collect()
}

fn block_events(evs: &[Event], at: P) -> Vec<(u8, u8)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::BlockEvent { pos, b0, b1, .. } if *pos == at => Some((*b0, *b1)),
            _ => None,
        })
        .collect()
}

fn arrow(w: &W, fire: bool) -> Proj {
    let mut p = Proj::new(PKind::Arrow, 900_901, [0.5, f64::from(Y) + 1.0, 0.5], stack("minecraft:arrow", 1).unwrap());
    p.owner = Some(w.p.s.uuid);
    if fire {
        p.fire = 100;
    }
    p
}

fn hit(at: P, face: usize, loc: [f64; 3]) -> BlockHit {
    BlockHit { loc, pos: at, face, inside: false, border: false }
}

fn tilt(w: &W, at: P) -> String {
    w.p.s.v().get(w.p.s.get(at), "tilt").unwrap_or("?").to_owned()
}

/// Big dripleaf: projectile → full tilt (sound, 100 ticks) then back; entity on top →
/// unstable, partial (10), full (10), back (100); redstone that resets it flat; survival;
/// leaf placed on top → stem; stem without a leaf broken at the tick; bone meal.
#[test]
fn big_dripleaf_tilts_and_grows() {
    let mut w = world("dripleaf");
    let at = [4, Y, 4];
    w.set(at, "minecraft:big_dripleaf[facing=east]");
    let _ = w.events();
    let s = w.p.s.get(at);
    let mut a = arrow(&w, false);
    w.p.s.block_projectile_hit(&mut a, hit(at, 1, [4.5, f64::from(Y) + 0.9375, 4.5]));
    let evs = w.events();
    assert_eq!(tilt(&w, at), "full", "projectile: full tilt");
    assert_eq!(sounds(&w, &evs, "minecraft:block.big_dripleaf.tilt_down").len(), 1);
    let mut evs = Vec::new();
    w.ticks(99, &mut evs);
    assert_eq!(tilt(&w, at), "full", "still tilted at 99 ticks");
    w.ticks(1, &mut evs);
    assert_eq!(tilt(&w, at), "none", "back to flat at 100 ticks");
    assert_eq!(sounds(&w, &evs, "minecraft:block.big_dripleaf.tilt_up").len(), 1);
    let _ = s;

    // Entity placed on top (passes through blocks): unstable with no sound, then the stages.
    w.p.s.dripleaf_entity_inside(at, true, f64::from(Y) + 0.6875);
    assert_eq!(tilt(&w, at), "none", "at the exact threshold: nothing");
    w.p.s.dripleaf_entity_inside(at, false, f64::from(Y) + 0.9375);
    assert_eq!(tilt(&w, at), "none", "in the air: nothing");
    w.p.s.dripleaf_entity_inside(at, true, f64::from(Y) + 0.9375);
    assert_eq!(tilt(&w, at), "unstable");
    let mut evs = Vec::new();
    w.ticks(10, &mut evs);
    assert_eq!(tilt(&w, at), "partial");
    w.ticks(10, &mut evs);
    assert_eq!(tilt(&w, at), "full");
    assert_eq!(sounds(&w, &evs, "minecraft:block.big_dripleaf.tilt_down").len(), 2);

    // Redstone next to it: immediate reset to flat.
    w.set([5, Y, 4], "minecraft:redstone_block");
    assert_eq!(tilt(&w, at), "none", "signal: flat");
    w.p.s.dripleaf_entity_inside(at, true, f64::from(Y) + 0.9375);
    assert_eq!(tilt(&w, at), "none", "powered: does not tilt");
    w.set([5, Y, 4], "minecraft:air");

    // Leaf placed on the leaf: the lower one becomes a stem (orientation kept).
    w.set(above(at), "minecraft:big_dripleaf[facing=east]");
    assert_eq!(name(&w, at), "minecraft:big_dripleaf_stem[facing=east,waterlogged=false]");
    // Upper leaf removed: the stem falls at the next tick (loot).
    w.set(above(at), "minecraft:air");
    let mut evs = Vec::new();
    w.ticks(1, &mut evs);
    assert_eq!(name(&w, at), "minecraft:air", "stem without a leaf broken");

    // Bone meal on the leaf: stem below, leaf above, same orientation.
    w.set(at, "minecraft:big_dripleaf[facing=south]");
    assert!(w.p.s.grow_with_bone_meal(at));
    assert_eq!(name(&w, at), "minecraft:big_dripleaf_stem[facing=south,waterlogged=false]");
    assert!(name(&w, above(at)).starts_with("minecraft:big_dripleaf[facing=south"), "{}", name(&w, above(at)));
    // On the stem: the top leaf goes up one notch.
    assert!(w.p.s.grow_with_bone_meal(at));
    assert!(name(&w, above(at)).starts_with("minecraft:big_dripleaf_stem"));
    assert!(name(&w, [4, Y + 2, 4]).starts_with("minecraft:big_dripleaf[facing=south"));
    // Ground removed: the bottom stem falls at the tick, then the whole column.
    w.set(below(at), "minecraft:air");
    let mut evs = Vec::new();
    w.ticks(4, &mut evs);
    assert_eq!(name(&w, [4, Y + 2, 4]), "minecraft:air", "column collapsed");
}

/// Powered note block: sound of the instrument (records category, volume 3, pitch of the note,
/// block center); mob head on top: imitation at pitch 1.
#[test]
fn note_block_plays_instrument_sound() {
    let mut w = world("notesound");
    let at = [6, Y, 6];
    w.set(at, "minecraft:note_block[note=6]");
    let _ = w.events();
    w.set([7, Y, 6], "minecraft:redstone_block");
    let mut evs = Vec::new();
    w.ticks(1, &mut evs);
    let inst = w.p.s.v().get(w.p.s.get(at), "instrument").unwrap_or("harp").to_owned();
    let s = sounds(&w, &evs, &format!("minecraft:block.note_block.{inst}"));
    assert_eq!(s.len(), 1, "one {inst} sound");
    assert_eq!(s[0].category, 2);
    assert_eq!(s[0].volume, 3.0);
    assert_eq!(s[0].pitch, 2f64.powf(-6.0 / 12.0) as f32);
    assert_eq!(s[0].pos, Some([6.5, f64::from(Y) + 0.5, 6.5]));
    assert_eq!(block_events(&evs, at), vec![(0, 0)], "block event sent");
    // Zombie head on top: imitation, pitch 1.
    w.set([7, Y, 6], "minecraft:air");
    w.set(above(at), "minecraft:zombie_head");
    w.set([7, Y, 6], "minecraft:redstone_block");
    let mut evs = Vec::new();
    w.ticks(1, &mut evs);
    let s = sounds(&w, &evs, "minecraft:block.note_block.imitate.zombie");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].pitch, 1.0);
}

/// Bell: rings on the rising edge of redstone (powered tracked); projectile from the right side
/// only (shooter player's statistic); survival on the ground; single ↔ double wall.
#[test]
fn bell_redstone_projectile_and_support() {
    let mut w = world("bell");
    let at = [8, Y, 8];
    w.set(at, "minecraft:bell[attachment=floor,facing=north]");
    let _ = w.events();
    w.set([9, Y, 8], "minecraft:redstone_block");
    assert!(w.p.s.v().get_bool(w.p.s.get(at), "powered"), "powered");
    let mut evs = Vec::new();
    w.ticks(1, &mut evs);
    assert_eq!(sounds(&w, &evs, "minecraft:block.bell.use").len(), 1, "rings on the rising edge");
    assert_eq!(block_events(&evs, at), vec![(1, Dir::North as u8)]);
    w.set([9, Y, 8], "minecraft:air");
    assert!(!w.p.s.v().get_bool(w.p.s.get(at), "powered"));
    let _ = w.events();

    // Projectile: from above → nothing; north face (axis of the ground orientation) → rings + statistic.
    let s = w.p.s.get(at);
    let mut a = arrow(&w, false);
    w.p.s.block_projectile_hit(&mut a, hit(at, 1, [8.5, f64::from(Y) + 1.0, 8.5]));
    w.p.s.block_projectile_hit(&mut a, hit(at, Dir::East as usize, [9.0, f64::from(Y) + 0.5, 8.5]));
    w.p.s.block_projectile_hit(&mut a, hit(at, Dir::North as usize, [8.5, f64::from(Y) + 0.9, 8.0]));
    let mut evs = w.events();
    w.ticks(1, &mut evs);
    assert!(sounds(&w, &evs, "minecraft:block.bell.use").is_empty(), "wrong sides: nothing");
    w.p.s.block_projectile_hit(&mut a, hit(at, Dir::North as usize, [8.5, f64::from(Y) + 0.5, 8.0]));
    let mut evs = w.events();
    w.ticks(1, &mut evs);
    assert_eq!(sounds(&w, &evs, "minecraft:block.bell.use").len(), 1);
    assert_eq!(w.p.s.prog.stats.get("minecraft:custom", "minecraft:bell_ring"), 1);
    let _ = s;

    // Ground removed: the bell falls.
    w.set(below(at), "minecraft:air");
    assert_eq!(name(&w, at), "minecraft:air", "bell on the ground without support");

    // Wall: single → double when a full block appears opposite, back to single turned towards the
    // block that remains.
    let b = [8, Y + 1, 12];
    w.set([8, Y + 1, 11], "minecraft:stone");
    w.set(b, "minecraft:bell[attachment=single_wall,facing=north]");
    w.set([8, Y + 1, 13], "minecraft:stone");
    assert_eq!(name(&w, b), "minecraft:bell[attachment=double_wall,facing=north,powered=false]");
    w.set([8, Y + 1, 11], "minecraft:air");
    assert_eq!(name(&w, b), "minecraft:bell[attachment=single_wall,facing=south,powered=false]");
    w.set([8, Y + 1, 13], "minecraft:air");
    assert_eq!(name(&w, b), "minecraft:air", "no more wall");
}

/// Bell placement by the player: single wall, double wall, ceiling, ceiling fallback on a
/// side face when the wall does not hold, refusal with no support at all.
#[test]
fn bell_placement_wall_ceiling_and_fallback() {
    let mut w = world("bellplace");
    let h = Y + 6;
    // North face of a stone in the air: single wall, the bell faces south (towards its support).
    w.set([20, h, 20], "minecraft:stone");
    assert!(w.p.s.place_item("minecraft:bell", [20, h, 20], Dir::North, [20.5, f64::from(h) + 0.5, 20.0]));
    assert_eq!(name(&w, [20, h, 19]), "minecraft:bell[attachment=single_wall,facing=south,powered=false]");
    // Stones on both sides of the axis: double wall.
    w.set([24, h, 20], "minecraft:stone");
    w.set([24, h, 18], "minecraft:stone");
    assert!(w.p.s.place_item("minecraft:bell", [24, h, 20], Dir::North, [24.5, f64::from(h) + 0.5, 20.0]));
    assert_eq!(name(&w, [24, h, 19]), "minecraft:bell[attachment=double_wall,facing=south,powered=false]");
    // Bottom face: ceiling.
    w.set([28, h, 20], "minecraft:stone");
    assert!(w.p.s.place_item("minecraft:bell", [28, h, 20], Dir::Down, [28.5, f64::from(h), 20.5]));
    assert!(name(&w, [28, h - 1, 20]).starts_with("minecraft:bell[attachment=ceiling,"), "{}", name(&w, [28, h - 1, 20]));
    // Side face of a fence (non-full face), stone above the cell: ceiling fallback.
    w.set([32, h, 20], "minecraft:oak_fence");
    w.set([32, h + 1, 19], "minecraft:stone");
    assert!(w.p.s.place_item("minecraft:bell", [32, h, 20], Dir::North, [32.5, f64::from(h) + 0.5, 20.0]));
    assert_eq!(name(&w, [32, h, 19]), "minecraft:bell[attachment=ceiling,facing=south,powered=false]");
    // Lone fence, nothing above or below: no placement.
    w.set([36, h, 20], "minecraft:oak_fence");
    assert!(!w.p.s.place_item("minecraft:bell", [36, h, 20], Dir::North, [36.5, f64::from(h) + 0.5, 20.0]));
    assert_eq!(name(&w, [36, h, 19]), "minecraft:air");
}

/// Chorus: the flower on End stone grows (plant connected below, event 1033); plant
/// deprived of its support broken at the tick, the flower follows; connections exposed for placement.
#[test]
fn chorus_grows_and_collapses() {
    let mut w = world("chorus");
    let base = [10, Y, 10];
    w.set(base, "minecraft:end_stone");
    let at = above(base);
    w.set(at, "minecraft:chorus_flower[age=0]");
    let _ = w.events();
    let s = w.p.s.get(at);
    w.p.s.random_tick(s, at);
    let evs = w.events();
    assert!(name(&w, at).starts_with("minecraft:chorus_plant"), "plant: {}", name(&w, at));
    assert!(w.p.s.v().get_bool(w.p.s.get(at), "up") && w.p.s.v().get_bool(w.p.s.get(at), "down"), "{}", name(&w, at));
    assert_eq!(name(&w, above(at)), "minecraft:chorus_flower[age=0]", "flower grown, same age");
    assert!(evs.iter().any(|e| matches!(e, Event::LevelEvent { kind: 1033, .. })), "1033");
    // Connections of one cell: plant below and flower above.
    let c = w.p.s.chorus_plant_state(at);
    assert_eq!(c, w.p.s.get(at));
    // Age 4 flower on any block (neither support nor plant nor air): dies (age 5, 1034).
    let lone = [12, Y + 5, 12];
    w.set(below(lone), "minecraft:stone");
    w.set(lone, "minecraft:chorus_flower[age=4]");
    let _ = w.events();
    let s = w.p.s.get(lone);
    w.p.s.random_tick(s, lone);
    assert_eq!(name(&w, lone), "minecraft:chorus_flower[age=5]");
    assert!(w.events().iter().any(|e| matches!(e, Event::LevelEvent { kind: 1034, .. })));
    // Support removed: plant broken at the next tick, then the flower.
    w.set(base, "minecraft:air");
    let mut evs = Vec::new();
    w.ticks(3, &mut evs);
    assert_eq!(name(&w, at), "minecraft:air", "plant collapsed");
    assert_eq!(name(&w, above(at)), "minecraft:air", "flower collapsed");
}

/// Budding amethyst: through random ticks, a small bud turned towards
/// the outside appears, then grows while staying turned to the same side.
#[test]
fn budding_amethyst_grows_buds() {
    let mut w = world("budding");
    let at = [3, Y + 4, 12];
    w.set(at, "minecraft:budding_amethyst");
    let s = w.p.s.get(at);
    for _ in 0..2000 {
        w.p.s.random_tick(s, at);
    }
    let mut stages = 0;
    for d in Dir::ALL {
        let n = name(&w, rel(at, d));
        if n.contains("amethyst_bud") || n.contains("amethyst_cluster") {
            assert!(n.contains(&format!("facing={}", d.name())), "{n} facing {}", d.name());
            stages += 1;
        }
    }
    assert!(stages >= 1, "at least one bud");
    assert!(
        Dir::ALL.iter().any(|&d| name(&w, rel(at, d)).starts_with("minecraft:amethyst_cluster")),
        "2000 ticks: at least one cluster"
    );
}

/// Head placed then powered: the "powered" state follows the signal.
#[test]
fn skull_follows_redstone() {
    let mut w = world("skull");
    let at = [2, Y, 12];
    w.set(at, "minecraft:dragon_head");
    w.set([3, Y, 12], "minecraft:redstone_block");
    assert!(w.p.s.v().get_bool(w.p.s.get(at), "powered"), "{}", name(&w, at));
    w.set([3, Y, 12], "minecraft:air");
    assert!(!w.p.s.v().get_bool(w.p.s.get(at), "powered"));
}

/// Amethyst hit by a projectile: chime (volume 1, pitch 0.5..1.7).
#[test]
fn amethyst_chimes_on_projectile() {
    let mut w = world("amethyst");
    for (i, n) in ["amethyst_block", "budding_amethyst", "amethyst_cluster[facing=up]"].iter().enumerate() {
        let at = [i as i32 * 2, Y, 2];
        w.set(at, &format!("minecraft:{n}"));
        let _ = w.events();
        let mut a = arrow(&w, false);
        w.p.s.block_projectile_hit(&mut a, hit(at, 1, [f64::from(at[0]) + 0.5, f64::from(Y) + 1.0, 2.5]));
        let evs = w.events();
        let s = sounds(&w, &evs, "minecraft:block.amethyst_block.chime");
        assert_eq!(s.len(), 1, "{n}");
        assert!((0.5..1.7).contains(&s[0].pitch), "{}", s[0].pitch);
    }
}

/// Simple candle cake: falls without support; flaming arrow → lit.
#[test]
fn plain_candle_cake_survival_and_fire() {
    let mut w = world("candlecake");
    let at = [12, Y + 1, 4];
    w.set(below(at), "minecraft:stone");
    w.set(at, "minecraft:candle_cake");
    let mut a = arrow(&w, true);
    w.p.s.block_projectile_hit(&mut a, hit(at, 1, [12.5, f64::from(Y) + 1.5, 4.5]));
    assert_eq!(name(&w, at), "minecraft:candle_cake[lit=true]", "lit by the flaming arrow");
    w.set(below(at), "minecraft:air");
    assert_eq!(name(&w, at), "minecraft:air", "without support");
}

/// Oxidation: an oxidized neighbour (final stage, which no longer ticks) counts as older.
#[test]
fn oxidized_neighbours_count_as_older() {
    let w = world("copper");
    let v = w.p.s.v();
    let st = |n: &str| v.default_state(w.p.s.bs.t.block_id(n));
    assert_eq!(w.p.s.copper_stage(st("oxidized_copper")), Some(3));
    assert_eq!(w.p.s.copper_stage(st("oxidized_cut_copper_stairs")), Some(3));
    assert_eq!(w.p.s.copper_stage(st("oxidized_copper_chest")), Some(3));
    assert_eq!(w.p.s.copper_stage(st("weathered_lightning_rod")), Some(2));
    assert_eq!(w.p.s.copper_stage(st("copper_block")), Some(0));
    assert_eq!(w.p.s.copper_stage(st("waxed_oxidized_copper")), None);
    assert_eq!(w.p.s.copper_stage(st("copper_torch")), None);
    assert_eq!(w.p.s.copper_stage(st("copper_wall_torch")), None);
    assert_eq!(w.p.s.copper_stage(st("copper_ore")), None);
    assert_eq!(w.p.s.copper_stage(st("raw_copper_block")), None);
}

/// A neighbouring wall copper torch does not stop exposed copper from aging.
#[test]
fn copper_wall_torch_does_not_stop_weathering() {
    let mut w = world("coppertorch");
    let at = [16, Y + 6, 16];
    w.set(at, "minecraft:exposed_copper");
    w.set([16, Y + 6, 17], "minecraft:copper_wall_torch[facing=south]");
    for _ in 0..5000 {
        let s = w.p.s.get(at);
        if name(&w, at) != "minecraft:exposed_copper" {
            break;
        }
        w.p.s.copper_random_tick(s, at);
    }
    assert_eq!(name(&w, at), "minecraft:weathered_copper");
}

/// Campfire: "signal" follows the hay bale below.
#[test]
fn campfire_signal_follows_hay_below() {
    let mut w = world("signal");
    let at = [14, Y + 1, 14];
    w.set(below(at), "minecraft:stone");
    w.set(at, "minecraft:campfire");
    w.set(below(at), "minecraft:hay_block");
    assert!(w.p.s.v().get_bool(w.p.s.get(at), "signal_fire"), "{}", name(&w, at));
    w.set(below(at), "minecraft:stone");
    assert!(!w.p.s.v().get_bool(w.p.s.get(at), "signal_fire"));
}

fn bees(w: &W) -> usize {
    w.p.s.mobs.list.iter().flatten().filter(|m| m.kind == crate::mobs::species::Kind::Bee && !m.removed).count()
}

const HIVE_BEE: &str = "{bees:[{entity_data:{id:\"minecraft:bee\"},min_ticks_in_hive:600,ticks_in_hive:0}]}";

/// Beehive: fire placed next to it → emergency exit; TNT that blows it up → bees released;
/// shears → "used" statistic.
#[test]
fn beehive_fire_explosion_and_stat() {
    let mut w = world("hive");
    let at = [4, Y, 12];
    w.cmd(&format!("setblock {} {} {} minecraft:beehive[facing=south]{HIVE_BEE}", at[0], at[1], at[2]));
    assert_eq!(bees(&w), 0);
    assert_eq!(crate::mobs::wild::hive::occupant_count(&w.p.s, at), 1, "bee in the hive");
    w.set([5, Y, 12], "minecraft:fire");
    assert_eq!(name(&w, [5, Y, 12]), "minecraft:fire[age=0,east=false,north=false,south=false,up=false,west=false]");
    assert_eq!(bees(&w), 1, "neighbouring fire: the bee leaves");

    // Far from the first bee and from the player (outside twice the radius).
    let at2 = [14, Y, 0];
    w.cmd(&format!("setblock {} {} {} minecraft:bee_nest[facing=south]{HIVE_BEE}", at2[0], at2[1], at2[2]));
    let before = bees(&w);
    w.p.s.tnt_explode(-1, [14.5, f64::from(Y) + 0.5, -0.5], 4.0, None, false, false);
    assert_eq!(name(&w, at2), "minecraft:air", "nest blown up");
    assert_eq!(bees(&w), before + 1, "TNT: the bee of the nest leaves");

    let at3 = [1, Y, 1];
    w.set(at3, "minecraft:beehive[facing=south,honey_level=5]");
    w.hold(stack("minecraft:shears", 1));
    let slot = 36 + w.p.s.held as usize;
    assert!(w.p.s.hive_use_item_on(slot, at3, "minecraft:shears"));
    assert_eq!(w.p.s.prog.stats.get("minecraft:used", "minecraft:shears"), 1);
}

/// Eye of Ender placed: the player standing on the frame (13/16) is lifted onto the eye (16/16).
#[test]
fn ender_eye_pushes_player_up() {
    let mut w = world("eye");
    let at = [6, Y, 14];
    w.set(at, "minecraft:end_portal_frame[facing=north]");
    w.p.s.pos = [6.5, f64::from(Y) + 13.0 / 16.0, 14.5];
    w.hold(stack("minecraft:ender_eye", 1));
    assert!(w.p.s.use_ender_eye(at));
    assert_eq!(w.p.s.pos[1], f64::from(Y) + 1.0, "lifted onto the eye");
}

/// Micro-bench (loaded machine, `--ignored`): oxidation stage by table versus by name, biome
/// of a block by index (with zoom) versus lookup by name (without zoom).
#[test]
#[ignore = "micro-bench"]
fn bench_g5f09_tables() {
    let w = world("bench");
    let s = &w.p.s;
    let states: Vec<u32> = ["oxidized_copper", "copper_block", "stone", "exposed_cut_copper_slab", "dirt"]
        .iter()
        .map(|n| s.v().default_state(s.bs.t.block_id(n)))
        .collect();
    let n = 2_000_000;
    let t = std::time::Instant::now();
    let mut acc = 0u32;
    for i in 0..n {
        acc = acc.wrapping_add(u32::from(s.copper_stage(states[i % 5]).unwrap_or(9)));
    }
    let table = t.elapsed().as_secs_f64() * 1e9 / n as f64;
    let t = std::time::Instant::now();
    for i in 0..n {
        let name = s.data.block_of(states[i % 5]).map_or("", |b| b.name.as_str());
        acc = acc.wrapping_add(u32::from(super::table::weather_stage(std::hint::black_box(name))));
    }
    let by_name = t.elapsed().as_secs_f64() * 1e9 / n as f64;
    eprintln!("oxidation stage: table {table:.1} ns, name {by_name:.1} ns ({acc})");
    let m = 200_000usize;
    let t = std::time::Instant::now();
    let mut k = 0usize;
    for i in 0..m {
        let p = [(i % 64) as i32 - 32, Y, ((i / 64) % 64) as i32 - 32];
        k += s.biome_of(p).map_or(0, |b| b.name.len());
    }
    let new = t.elapsed().as_secs_f64() * 1e9 / m as f64;
    let t = std::time::Instant::now();
    for i in 0..m {
        let p = [(i % 64) as i32 - 32, Y, ((i / 64) % 64) as i32 - 32];
        let c = s.chunks.get(&(p[0] >> 4, p[2] >> 4)).and_then(Option::as_ref);
        let ly = p[1] - s.dim.height.min_y;
        let b = c.and_then(|c| c.sections.get((ly >> 4) as usize)).map(|sec| {
            sec.biomes.get(((((ly & 15) >> 2) << 4) | (((p[2] & 15) >> 2) << 2) | ((p[0] & 15) >> 2)) as usize)
        });
        let name = b.and_then(|b| s.states.biome_names.get(b as usize));
        k += name.and_then(|nm| s.data.biomes.iter().find(|x| &x.name == nm)).map_or(0, |b| b.name.len());
    }
    let old = t.elapsed().as_secs_f64() * 1e9 / m as f64;
    eprintln!("biome of a block: index + zoom {new:.1} ns, name without zoom {old:.1} ns ({k})");
}

/// Biome zoom on the server side: a block whose blurred zoom falls in the neighbouring quart takes
/// the biome of that quart (snow, freeze, rain).
#[test]
fn server_biome_uses_zoom() {
    let mut w = world("zoom");
    // Quart (2, ., 2) of blocks 8..11 in snowy plains over the whole useful height.
    w.cmd(&format!("fillbiome 8 {} 8 11 {} 11 minecraft:snowy_plains", Y - 4, Y + 7));
    let seed = w.p.s.wgs.zoom_seed;
    // Blocks whose own quart and zoomed quart differ on either side of the boundary.
    let inside = |q: (i32, i32)| q == (2, 2);
    let mut checked = 0;
    for x in 0..20 {
        for z in 0..20 {
            let (qx, qy, qz) = rc_worldgen::biome::zoom(seed, x, Y, z);
            assert!((-16..=-14).contains(&qy), "vertical quart filled");
            let (zoomed, own) = (inside((qx, qz)), inside((x >> 2, z >> 2)));
            if zoomed != own {
                let want = if zoomed { "minecraft:snowy_plains" } else { "minecraft:plains" };
                let b = w.p.s.biome_of([x, Y, z]).map(|b| b.name.clone());
                assert_eq!(b.as_deref(), Some(want), "block {:?}", [x, Y, z]);
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "at least one block whose zoom changes quart");
}
