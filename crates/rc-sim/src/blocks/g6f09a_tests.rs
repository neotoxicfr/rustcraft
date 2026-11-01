//! G6-F09a: sulfur spike (common speleothem rules: placement, survival, fall,
//! growth) and active sulfur (state, block entity: nausea, countdown, launch). Fresh flat
//! world from G6-F01: a missing 26.3 cache or client jar makes the test FAIL.

use rc_client::Event;

use super::g6f01_tests::{W, Y, stack, world};
use super::{Dir, P};
use crate::blocks::level::BeTick;
use crate::mobs::species::Kind;

const SPIKE: &str = "minecraft:sulfur_spike";

fn prop(w: &W, at: P, name: &str) -> String {
    let s = w.p.s.get(at);
    w.p.s.v().get(s, name).unwrap_or("?").to_owned()
}

fn block_events(evs: &[Event], at: P) -> usize {
    evs.iter().filter(|e| matches!(e, Event::BlockEvent { pos, b0: 0, b1: 0, .. } if *pos == at)).count()
}

/// Player placement: looking up under the sulfur → tip down; tip lengthened from
/// below (the upper piece becomes a trunk); looking up on the ground → fall back to a
/// stalagmite; two tips facing each other merge; no placement without support.
#[test]
fn sulfur_spike_placement_follows_support_and_gaze() {
    let mut w = world("g6f09a-place");
    w.p.s.pos = [10.5, f64::from(Y), 12.5];
    let roof = [12, Y + 4, 12];
    w.set(roof, "minecraft:sulfur");
    w.hold(stack(SPIKE, 64));
    w.p.s.pitch = -40.0; // looking up
    w.click(roof, Dir::Down, [0.5, 0.0, 0.5]);
    let a = [12, Y + 3, 12];
    assert_eq!(w.p.s.state_name(w.p.s.get(a)), SPIKE, "spike placed under the sulfur");
    assert_eq!(prop(&w, a, "vertical_direction"), "down", "tip pointing down");
    assert_eq!(prop(&w, a, "thickness"), "tip");
    w.click(a, Dir::Down, [0.5, 0.0, 0.5]);
    let b = [12, Y + 2, 12];
    assert_eq!(prop(&w, b, "vertical_direction"), "down");
    assert_eq!(prop(&w, b, "thickness"), "tip");
    assert_eq!(prop(&w, a, "thickness"), "frustum", "the piece above the tip");
    // On the ground, still looking up: no support above, fall back to pointing up.
    let floor = [12, Y - 1, 12];
    w.click(floor, Dir::Up, [0.5, 1.0, 0.5]);
    let c = [12, Y, 12];
    assert_eq!(prop(&w, c, "vertical_direction"), "up", "fallback stalagmite");
    // Between the stalagmite and the upper tip: facing each other, merge (player standing).
    w.click(c, Dir::Up, [0.5, 1.0, 0.5]);
    let d = [12, Y + 1, 12];
    assert_eq!(prop(&w, d, "thickness"), "tip_merge", "merge with the tip facing it");
    // Without support (nothing sturdy above or below): refused.
    let pillar = [10, Y + 2, 14];
    w.set(pillar, "minecraft:stone");
    w.click(pillar, Dir::West, [0.0, 0.5, 0.5]);
    assert_eq!(w.p.s.state_name(w.p.s.get([9, Y + 2, 14])), "minecraft:air", "floating spike placed");
}

/// Sulfur removed: the whole stalactite falls; only the tip hurts (6 per block, 40 at
/// most); a stalagmite without ground is broken.
#[test]
fn sulfur_stalactite_falls_and_stalagmite_breaks() {
    let mut w = world("g6f09a-fall");
    let roof = [12, Y + 4, 12];
    w.set(roof, "minecraft:sulfur");
    w.set([12, Y + 3, 12], "minecraft:sulfur_spike[vertical_direction=down,thickness=frustum]");
    w.set([12, Y + 2, 12], "minecraft:sulfur_spike[vertical_direction=down,thickness=tip]");
    let mut evs = Vec::new();
    w.set(roof, "minecraft:air");
    let mut spawned = false;
    for _ in 0..4 {
        w.ticks(1, &mut evs);
        if !w.p.s.bs.entities.list.is_empty() {
            spawned = true;
            break;
        }
    }
    assert!(spawned, "no fall");
    let list = &w.p.s.bs.entities.list;
    assert_eq!(list.len(), 2, "both pieces fall");
    assert_eq!(list[0].hurt, None, "the trunk does not hurt");
    assert_eq!(list[1].hurt, Some((6.0, 40)), "the tip hurts");
    assert_eq!(w.p.s.state_name(w.p.s.get([12, Y + 3, 12])), "minecraft:air");
    // Stalagmite on a removed ground: broken on the next tick.
    w.set([5, Y, 5], "minecraft:stone");
    w.set([5, Y + 1, 5], "minecraft:sulfur_spike[vertical_direction=up,thickness=tip]");
    w.set([5, Y, 5], "minecraft:air");
    w.ticks(2, &mut evs);
    assert_eq!(w.p.s.state_name(w.p.s.get([5, Y + 1, 5])), "minecraft:air", "stalagmite without ground");
}

/// Random ticks at the top of stalactites under sulfur: the tip lengthens (3 blocks at
/// most for the spike: the tip is only searched over 2 blocks); while it is short, half
/// of the growths make a stalagmite appear on the ground (six columns: at least one has one).
#[test]
fn sulfur_spike_grows_from_random_ticks() {
    let mut w = world("g6f09a-grow");
    let cols: Vec<i32> = (0..6).map(|i| 2 + 2 * i).collect();
    for &x in &cols {
        w.set([x, Y + 7, 12], "minecraft:sulfur");
        w.set([x, Y + 6, 12], "minecraft:sulfur_spike[vertical_direction=down,thickness=tip]");
    }
    for _ in 0..30_000 {
        for &x in &cols {
            let top = [x, Y + 6, 12];
            let s = w.p.s.get(top);
            w.p.s.random_tick(s, top);
        }
    }
    let spike = |w: &W, x: i32, y: i32| w.p.s.state_name(w.p.s.get([x, y, 12])) == SPIKE;
    let mut floors = 0;
    for &x in &cols {
        let hanging = (0..7)
            .take_while(|&i| spike(&w, x, Y + 6 - i) && prop(&w, [x, Y + 6 - i, 12], "vertical_direction") == "down")
            .count();
        // 3 blocks, or fewer when the stalagmite has joined it (merged tips: no more growth).
        let low = [x, Y + 7 - hanging as i32, 12];
        assert!(
            hanging == 3 || (hanging > 0 && prop(&w, low, "thickness") == "tip_merge"),
            "stalactite of {hanging} at x = {x}"
        );
        assert!(!spike(&w, x, Y + 3) || prop(&w, [x, Y + 3, 12], "vertical_direction") == "up", "more than 3 blocks");
        if spike(&w, x, Y) {
            assert_eq!(prop(&w, [x, Y, 12], "vertical_direction"), "up");
            floors += 1;
        }
    }
    assert!(floors > 0, "no stalagmite grew");
    // Without sulfur above, nothing grows.
    let other = [5, Y + 4, 5];
    w.set([5, Y + 5, 5], "minecraft:stone");
    w.set(other, "minecraft:sulfur_spike[vertical_direction=down,thickness=tip]");
    for _ in 0..20_000 {
        let s = w.p.s.get(other);
        w.p.s.random_tick(s, other);
    }
    assert_eq!(w.p.s.state_name(w.p.s.get([5, Y + 3, 5])), "minecraft:air", "growth under the stone");
}

/// Active sulfur state from the water above and the block below; ticker only outside the dry
/// state; `countdown` at -1 at creation.
#[test]
fn potent_sulfur_state_follows_water_and_floor() {
    let mut w = world("g6f09a-state");
    let at = [8, Y, 8];
    w.set(below(at), "minecraft:magma_block");
    w.set(at, "minecraft:potent_sulfur");
    assert_eq!(prop(&w, at, "potent_sulfur_state"), "dry");
    assert!(!matches!(w.p.s.bs.be_ticks.get(&at), Some((_, BeTick::PotentSulfur))), "dry: no ticker");
    let cd = w.p.s.be_get(at).and_then(|b| crate::nbtio::int(b, "countdown"));
    assert_eq!(cd.map(|v| v as i32), Some(-1), "initial countdown");
    w.set(above(at), "minecraft:water");
    assert_eq!(prop(&w, at, "potent_sulfur_state"), "dormant", "water above, magma below");
    assert!(matches!(w.p.s.bs.be_ticks.get(&at), Some((_, BeTick::PotentSulfur))), "ticker set");
    w.set(below(at), "minecraft:stone");
    assert_eq!(prop(&w, at, "potent_sulfur_state"), "wet");
    w.set(above(at), "minecraft:air");
    assert_eq!(prop(&w, at, "potent_sulfur_state"), "dry");
    assert!(!matches!(w.p.s.bs.be_ticks.get(&at), Some((_, BeTick::PotentSulfur))), "ticker removed");
}

fn below(p: P) -> P {
    [p[0], p[1] - 1, p[2]]
}

fn above(p: P) -> P {
    [p[0], p[1] + 1, p[2]]
}

/// Lava below: continuous geyser (block event and start sound), +0.2 launch of a
/// mob in the column, nothing for the Ender dragon.
#[test]
fn potent_sulfur_continuous_geyser_launches_entities() {
    let mut w = world("g6f09a-geyser");
    let at = [8, Y, 8];
    w.set(below(at), "minecraft:lava");
    w.set(at, "minecraft:potent_sulfur");
    let _ = w.events();
    w.set(above(at), "minecraft:water");
    assert_eq!(prop(&w, at, "potent_sulfur_state"), "continuous");
    let mut evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:block.potent_sulfur.geyser_continuous_eruption"), 1, "start sound");
    w.cmd(&format!("summon minecraft:pig 8.5 {} 8.5 {{NoAI:1b}}", Y + 1));
    let pig = w.p.s.mobs.of_kind(Kind::Pig).next().expect("pig").id;
    let i = w.p.s.mob_index(pig).expect("pig");
    w.p.s.mobs.list[i].as_mut().expect("pig").vel = [0.0; 3];
    w.ticks(1, &mut evs);
    assert!(block_events(&evs, at) >= 1, "start block event");
    let vy = w.p.s.mobs.list[i].as_ref().expect("pig").vel[1];
    assert!((vy - f64::from(0.2f32)).abs() < 1e-9, "pig launch: {vy}");
    // Already fast enough (0.3 + 0.1 × 1 block of water): no more push.
    w.p.s.mobs.list[i].as_mut().expect("pig").vel = [0.0, 0.5, 0.0];
    w.ticks(1, &mut evs);
    assert_eq!(w.p.s.mobs.list[i].as_ref().expect("pig").vel[1], 0.5);
}

/// Wet: 80-tick ambient nausea for a player whose eyes are above the water.
#[test]
fn potent_sulfur_gives_nausea_above_water() {
    let mut w = world("g6f09a-nausea");
    let at = [8, Y, 8];
    w.set(at, "minecraft:potent_sulfur");
    w.cmd(&format!("fill 7 {} 7 9 {} 9 minecraft:water", Y + 1, Y + 1));
    assert_eq!(prop(&w, at, "potent_sulfur_state"), "wet");
    w.p.s.pos = [8.5, f64::from(Y + 1), 8.5];
    let mut evs = Vec::new();
    w.ticks(10, &mut evs);
    let fx = w.p.s.life.fx.list.iter().find(|e| e.id == "minecraft:nausea").expect("nausea applied").clone();
    assert!(fx.ambient && fx.dur > 60 && fx.dur <= 80, "80-tick ambient nausea: {fx:?}");
}

/// Bench (loaded machine accepted): random tick of a spike and of pointed dripstone with no
/// possible growth, against that of a stone; block entity tick of a continuous geyser with 0 then 200
/// mobs in the world (best of 5).
#[test]
#[ignore = "bench"]
fn bench_speleothem_random_tick_and_geyser() {
    let mut w = world("g6f09a-bench");
    let (spike, drip, stone) = ([4, Y + 2, 4], [6, Y + 2, 4], [8, Y + 2, 4]);
    w.set(above(spike), "minecraft:stone");
    w.set(spike, "minecraft:sulfur_spike[vertical_direction=down,thickness=tip]");
    w.set(above(drip), "minecraft:stone");
    w.set(drip, "minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]");
    w.set(stone, "minecraft:stone");
    let at = [12, Y, 12];
    w.set(below(at), "minecraft:lava");
    w.set(at, "minecraft:potent_sulfur");
    w.set(above(at), "minecraft:water");
    let n = 200_000u32;
    let best = |w: &mut W, n: u32, f: &dyn Fn(&mut W)| {
        (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                for _ in 0..n {
                    f(w);
                }
                t.elapsed().as_secs_f64() * 1e9 / f64::from(n)
            })
            .fold(f64::MAX, f64::min)
    };
    let rt = |p: P| move |w: &mut W| {
        let s = w.p.s.get(p);
        w.p.s.random_tick(s, p);
    };
    let ns_spike = best(&mut w, n, &rt(spike));
    let ns_drip = best(&mut w, n, &rt(drip));
    let ns_stone = best(&mut w, n, &rt(stone));
    let geyser = |w: &mut W| w.p.s.potent_sulfur_tick(at);
    let g0 = best(&mut w, 20_000, &geyser);
    // Mobs outside the column (more than 8 blocks away), as in a populated cave.
    for i in 0..200 {
        w.cmd(&format!("summon minecraft:pig {} {} {} {{NoAI:1b}}", 22 + i % 14, Y + 2, -20 + i / 14));
    }
    let g200 = best(&mut w, 20_000, &geyser);
    eprintln!(
        "bench G6-F09a: random tick spike {ns_spike:.1} ns, pointed {ns_drip:.1} ns, stone {ns_stone:.1} ns; \
         continuous geyser {g0:.1} ns (0 mobs), {g200:.1} ns (200 mobs)"
    );
}

/// Magma below: countdown drawn (1 block of water: 14 to 29 after the first examination),
/// eruption (sound, block event) then back to dormant (`block_deactivate`).
#[test]
fn potent_sulfur_periodic_geyser_cycles() {
    let mut w = world("g6f09a-cycle");
    let at = [8, Y, 8];
    w.set(below(at), "minecraft:magma_block");
    w.set(at, "minecraft:potent_sulfur");
    w.set(above(at), "minecraft:water");
    assert_eq!(prop(&w, at, "potent_sulfur_state"), "dormant");
    let mut evs = Vec::new();
    while w.p.s.time % 20 != 19 {
        w.ticks(1, &mut evs);
    }
    w.ticks(1, &mut evs);
    let cd = w.p.s.be_get(at).and_then(|b| crate::nbtio::int(b, "countdown")).expect("countdown") as i32;
    assert!((14..=29).contains(&cd), "countdown drawn: {cd}");
    evs.clear();
    w.ticks(20 * 31, &mut evs);
    assert!(w.sounds(&evs, "minecraft:block.potent_sulfur.geyser_eruption") >= 1, "eruption");
    assert!(block_events(&evs, at) >= 1, "eruption block event");
    let seen_dormant_again = prop(&w, at, "potent_sulfur_state") == "dormant" || {
        w.ticks(20 * 3, &mut evs);
        prop(&w, at, "potent_sulfur_state") == "dormant"
    };
    assert!(seen_dormant_again, "end of eruption");
}
