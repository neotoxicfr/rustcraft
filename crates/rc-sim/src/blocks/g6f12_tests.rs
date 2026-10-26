//! G6-F12: common block entity tick test (border), conduit, opening of the Ender chest and the
//! Shulker box. Fresh flat world (that of G6-F01): no external data other than the 26.3 cache
//! and the client jar, whose absence makes the test FAIL.

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::P;
use super::conduit::FRAME;
use super::g6f01_tests::{W, Y, world};
use crate::mobs::species::Kind;

fn sounds(w: &W, evs: &[Event], name: &str) -> usize {
    let id = w.p.s.sound_id(name).expect("known sound");
    evs.iter()
        .filter(|e| matches!(e, Event::Sound(s) if matches!(s.sound, rc_proto_v777::play::SoundRef::Id(i) if i == id)))
        .count()
}

/// One game tick: time, blocks and block entities, furnaces, sending the changes.
fn tick(w: &mut W, out: &mut Vec<Event>) {
    w.p.s.time += 1;
    w.p.s.tick_world_blocks();
    w.p.s.furnaces_tick();
    w.p.s.flush_block_changes();
    out.extend(w.p.take_events());
}

fn cooking(w: &W, at: P) -> i32 {
    match w.p.s.be_get(at).and_then(|b| b.get("CookingTimes")) {
        Some(Nbt::IntArray(v)) => v[0],
        _ => panic!("campfire without CookingTimes"),
    }
}

/// Outside the border, neither the campfire (common list of tickers) nor the furnace (own loop)
/// tick; with the border widened, both resume.
#[test]
fn block_entities_stop_ticking_outside_the_world_border() {
    let mut w = world("g6f12-border");
    w.p.s.loaded = true;
    let (fire, oven) = ([10, Y, 10], [12, Y, 10]);
    w.cmd(&format!(
        "setblock 10 {Y} 10 minecraft:campfire[lit=true]{{Items:[{{Slot:0b,id:\"minecraft:beef\",count:1}}],\
         CookingTimes:[I;0,0,0,0],CookingTotalTimes:[I;600,600,600,600]}}"
    ));
    w.cmd(&format!("setblock 12 {Y} 10 minecraft:furnace{{lit_time_remaining:100,lit_total_time:100}}"));
    let lit = |w: &W| w.p.s.life.furnaces.get(&oven).expect("simulated furnace").lit_remaining;
    assert_eq!(lit(&w), 100, "furnace: reserve read back");
    // Border of 4 blocks around the origin: both blocks are outside.
    w.cmd("worldborder center 0 0");
    w.cmd("worldborder set 4");
    let mut evs = Vec::new();
    for _ in 0..5 {
        tick(&mut w, &mut evs);
    }
    assert_eq!(cooking(&w, fire), 0, "campfire outside the border: cooking advanced");
    assert_eq!(lit(&w), 100, "furnace outside the border: fuel burned");
    w.cmd("worldborder set 1000");
    for _ in 0..5 {
        tick(&mut w, &mut evs);
    }
    assert_eq!(cooking(&w, fire), 5, "campfire inside the border");
    assert_eq!(lit(&w), 95, "furnace inside the border");
}

/// Bench: cost of the common test per block entity (border then tick range) against the previous
/// tick range test alone, and of a full examination of the conduit frame (best of 5).
#[test]
#[ignore = "bench"]
fn bench_be_ticking_and_conduit_frame() {
    let mut w = world("g6f12-bench");
    let c = [8, Y + 2, 8];
    w.cmd(&format!("fill 7 {} 7 9 {} 9 minecraft:water", Y + 1, Y + 3));
    w.set(c, "minecraft:conduit[waterlogged=true]");
    for o in &FRAME {
        w.set([c[0] + o[0], c[1] + o[1], c[2] + o[2]], "minecraft:prismarine");
    }
    let n = 1_000_000u32;
    let best = |f: &dyn Fn(u32) -> bool| {
        (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                let mut k = 0u32;
                for i in 0..n {
                    k += u32::from(f(i));
                }
                std::hint::black_box(k);
                t.elapsed().as_secs_f64() * 1e9 / f64::from(n)
            })
            .fold(f64::MAX, f64::min)
    };
    let s = &w.p.s;
    let at = |i: u32| [(i % 16) as i32, Y, (i / 16 % 16) as i32];
    let new = best(&|i| s.be_ticking(at(i)));
    let old = best(&|i| s.ticking_at(at(i)));
    let frame = best(&|_| s.conduit_frame(c) == 42);
    eprintln!("bench G6-F12: be_ticking {new:.1} ns, ticking_at {old:.1} ns, conduit frame {frame:.1} ns");
}

/// Conduit: nothing under 16 frame blocks; at 16, activation sound and effect on the player in
/// the water; at 42, wet hostile target hurt for 4, stored in `Target` and sent back to the
/// client; dried-out heart: deactivation sound, no more hit.
#[test]
fn conduit_activates_gives_power_and_hunts() {
    let mut w = world("g6f12-conduit");
    w.p.s.loaded = true;
    let c = [8, Y + 2, 8];
    w.cmd(&format!("fill 7 {} 7 9 {} 9 minecraft:water", Y + 1, Y + 3));
    w.set(c, "minecraft:conduit[waterlogged=true]");
    let frame = |w: &mut W, n: std::ops::Range<usize>| {
        for o in &FRAME[n] {
            w.set([c[0] + o[0], c[1] + o[1], c[2] + o[2]], "minecraft:prismarine");
        }
    };
    frame(&mut w, 0..15);
    // Player in the water at 6 blocks; zombie in the water at 4 blocks (swimming kept by its tick).
    w.set([8, Y, 14], "minecraft:water");
    w.p.s.pos = [8.5, f64::from(Y), 14.5];
    w.set([8, Y, 4], "minecraft:water");
    w.cmd(&format!("summon minecraft:zombie 8.5 {Y} 4.5 {{NoAI:1b}}"));
    let zid = w.p.s.mobs.of_kind(Kind::Zombie).next().expect("zombie").id;
    let zi = w.p.s.mob_index(zid).expect("zombie");
    let (zuuid, zhealth) = {
        let z = w.p.s.mobs.list[zi].as_mut().expect("zombie");
        z.in_water = true;
        (z.uuid, z.health)
    };
    let health = |w: &W| w.p.s.mobs.get_uuid(zuuid).expect("zombie").health;
    let target = |w: &W| w.p.s.be_get(c).and_then(|b| crate::nbtio::uuid(b.get("Target")));
    let until = |w: &mut W, at: i64, evs: &mut Vec<Event>| {
        while w.p.s.time < at {
            tick(w, evs);
        }
    };
    w.p.s.time = 39;
    let mut evs = Vec::new();
    // 15 blocks: inactive.
    until(&mut w, 40, &mut evs);
    assert_eq!(w.p.s.conduit_frame(c), 15);
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.activate"), 0, "15 blocks: activated");
    assert!(!w.p.s.life.fx.has("minecraft:conduit_power"), "15 blocks: effect given");
    // 16 blocks: active, effect, no hunt.
    frame(&mut w, 15..16);
    evs.clear();
    until(&mut w, 80, &mut evs);
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.activate"), 1, "16 blocks: activation sound");
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.ambient"), 1, "80-tick ambient");
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.ambient.short"), 1, "short ambient");
    let e = w.p.s.life.fx.get("minecraft:conduit_power").expect("conduit effect");
    assert_eq!((e.dur, e.amp, e.ambient, e.visible), (260, 0, true, true));
    assert_eq!(health(&w), zhealth, "16 blocks: hunt");
    assert_eq!(target(&w), None);
    // Full frame: hunt.
    frame(&mut w, 16..42);
    assert_eq!(w.p.s.conduit_frame(c), 42);
    evs.clear();
    until(&mut w, 120, &mut evs);
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.activate"), 0, "already active");
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.attack.target"), 1, "hit sound");
    assert_eq!(health(&w), zhealth - 4.0, "4 magic damage");
    assert_eq!(target(&w), Some(zuuid), "target stored in Target");
    let tag = w.p.s.be_update_tag(c, w.p.s.get(c)).expect("conduit update");
    assert_eq!(crate::nbtio::uuid(tag.get("Target")), Some(zuuid), "target sent back to the client");
    // Dried-out heart: deactivation, target kept without a hit.
    w.set([9, Y + 1, 9], "minecraft:stone");
    evs.clear();
    until(&mut w, 160, &mut evs);
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.deactivate"), 1, "deactivation sound");
    assert_eq!(sounds(&w, &evs, "minecraft:block.conduit.attack.target"), 0);
    assert_eq!(health(&w), zhealth - 4.0);
}

/// Ender chest under a conductive block, Shulker box whose lid hits a block: nothing opens and
/// no window number is taken; obstacle removed or leaving room for the lid (top slab):
/// the menu opens.
#[test]
fn ender_chest_and_shulker_box_open_only_when_free() {
    let mut w = world("g6f12-open");
    let (ender, box_) = ([4, Y, 4], [6, Y, 4]);
    w.set(ender, "minecraft:ender_chest");
    w.set([4, Y + 1, 4], "minecraft:stone");
    w.set(box_, "minecraft:shulker_box[facing=up]");
    w.set([6, Y + 1, 4], "minecraft:smooth_stone_slab[type=bottom]");
    let opens = |w: &mut W, at: P| {
        w.p.s.life.menu = None;
        let before = w.p.s.life.counter;
        assert!(w.p.s.use_block(at), "click not taken");
        let open = w.p.s.life.menu.is_some();
        if !open {
            assert_eq!(w.p.s.life.counter, before, "window number taken without a menu");
        }
        w.p.s.life.menu = None;
        open
    };
    assert!(!opens(&mut w, ender), "Ender chest under the stone opened");
    assert!(!opens(&mut w, box_), "Shulker box under a bottom slab opened");
    w.set([4, Y + 1, 4], "minecraft:glass");
    w.set([6, Y + 1, 4], "minecraft:smooth_stone_slab[type=top]");
    assert!(opens(&mut w, ender), "Ender chest under the glass closed");
    assert!(opens(&mut w, box_), "Shulker box under a top slab closed");
    // Lid turned east against a block: closed; spectator: opens anyway.
    let east = [6, Y, 7];
    w.set(east, "minecraft:shulker_box[facing=east]");
    w.set([7, Y, 7], "minecraft:stone");
    assert!(!opens(&mut w, east), "box turned towards the stone opened");
    w.p.s.game_mode = 3;
    assert!(opens(&mut w, east), "spectator: box closed");
    w.p.s.game_mode = 0;
    // Shulker (solid entity) placed on a free box: closed; killed: open.
    let top = [9, Y, 4];
    w.set(top, "minecraft:shulker_box[facing=up]");
    assert!(opens(&mut w, top), "free box closed");
    w.cmd(&format!("summon minecraft:shulker 9 {} 4 {{NoAI:1b}}", Y + 1));
    let sid = w.p.s.mobs.of_kind(Kind::Shulker).next().expect("shulker").id;
    assert!(!opens(&mut w, top), "box under a shulker opened");
    let si = w.p.s.mob_index(sid).expect("shulker");
    w.p.s.mobs.list[si].as_mut().expect("shulker").health = 0.0;
    assert!(opens(&mut w, top), "box under a dead shulker closed");
}

/// Bell rung near a pillager: outside the border, neither resonance nor glowing;
/// with the border widened, the bell (kept) resumes its count.
#[test]
fn bell_stops_ticking_outside_the_world_border() {
    let mut w = world("g6f12-bell");
    w.p.s.loaded = true;
    let bell = [10, Y, 10];
    w.set(bell, "minecraft:bell[attachment=floor,facing=north]");
    w.cmd(&format!("summon minecraft:pillager 12.5 {Y} 10.5 {{NoAI:1b}}"));
    let pid = w.p.s.mobs.of_kind(Kind::Pillager).next().expect("pillager").id;
    let glowing = |w: &W| {
        let i = w.p.s.mob_index(pid).expect("pillager");
        w.p.s.mobs.list[i].as_ref().expect("pillager").x.effects.iter().any(|e| e.id == "minecraft:glowing")
    };
    w.cmd("worldborder center 0 0");
    w.cmd("worldborder set 4");
    w.p.s.bell_rung(bell);
    let mut evs = Vec::new();
    for _ in 0..60 {
        w.p.s.bells_tick();
        evs.extend(w.p.take_events());
    }
    assert_eq!(sounds(&w, &evs, "minecraft:block.bell.resonate"), 0, "bell outside the border: resonance");
    assert!(!glowing(&w), "bell outside the border: pillager glowing");
    assert_eq!(w.p.s.mobs.bells.len(), 1, "bell forgotten outside the border");
    w.cmd("worldborder set 1000");
    for _ in 0..60 {
        w.p.s.bells_tick();
        evs.extend(w.p.take_events());
    }
    assert_eq!(sounds(&w, &evs, "minecraft:block.bell.resonate"), 1, "bell inside the border");
    assert!(glowing(&w), "bell inside the border: no glowing");
}
