//! Crafter crafting on its scheduled tick: result and remainders out through the front (into
//! the container there, one at a time into another crafter, thrown otherwise), the `crafting`
//! face counted down by the block entity, refusal without a recipe, placement while powered and
//! the comparator count. Before, a powered crafter crafted nothing.

use rc_proto::nbt::Nbt;

use super::test_area::{Area, level_events, open};
use super::{Dir, P};

const CRAFTER: P = [110, -59, 110];
/// South of the crafter: a redstone block placed there is the rising edge.
const POWER: P = [110, -59, 111];

fn crafting(a: &Area) -> bool {
    a.name(CRAFTER).contains("crafting=true")
}

fn countdown(a: &Area) -> Option<i64> {
    a.p.s.be_get(CRAFTER).and_then(|b| crate::nbtio::int(b, "crafting_ticks_remaining"))
}

/// One log, one pulse: 4 planks thrown east (click 1049, smoke 2010 towards east = 5), one log
/// used; the `crafting` face shows from the craft tick until 5 ticks later; no second craft
/// without a new pulse.
#[test]
fn crafter_crafts_once_per_pulse_and_throws_the_result() {
    let Some(mut a) = open("crafter-throw") else { return };
    a.set(CRAFTER, "minecraft:crafter[orientation=east_up]");
    a.put(CRAFTER, &[(0, "minecraft:oak_log", 2)]);
    let _ = a.ticks(2);
    a.set(POWER, "minecraft:redstone_block");
    assert!(a.name(CRAFTER).contains("triggered=true"), "rising edge not seen");
    let evs = a.ticks(3);
    assert!(!level_events(&evs).iter().any(|e| e.0 == 1049), "crafted before 4 ticks");
    let evs = a.ticks(1);
    let events = level_events(&evs);
    assert!(events.contains(&(1049, 0)), "no click of the thrown result: {events:?}");
    assert!(events.contains(&(2010, Dir::East as i32)), "no smoke towards the front: {events:?}");
    assert_eq!(a.dropped("minecraft:oak_planks"), 4, "result thrown");
    assert_eq!(a.held(CRAFTER, "minecraft:oak_log"), 1, "one log used");
    assert!(crafting(&a), "crafting face missing");
    assert_eq!(countdown(&a), Some(5), "block entity countdown after its first tick");
    let _ = a.ticks(4);
    assert!(crafting(&a), "crafting face gone too early");
    let _ = a.ticks(1);
    assert!(!crafting(&a), "crafting face kept after 6 ticks");
    assert_eq!(countdown(&a), Some(0), "countdown kept in the block entity (saved)");
    let _ = a.ticks(10);
    assert_eq!(a.dropped("minecraft:oak_planks"), 4, "crafted again without a new pulse");
}

/// A cake into the chest in front, with its three bucket remainders: all go in, silently (no
/// click, no smoke, nothing thrown), and the grid is used up.
#[test]
fn crafter_fills_the_chest_in_front_with_result_and_remainders() {
    let Some(mut a) = open("crafter-chest") else { return };
    let chest: P = [111, -59, 110];
    a.set(CRAFTER, "minecraft:crafter[orientation=east_up]");
    a.set(chest, "minecraft:chest[facing=west]");
    a.put(
        CRAFTER,
        &[
            (0, "minecraft:milk_bucket", 1),
            (1, "minecraft:milk_bucket", 1),
            (2, "minecraft:milk_bucket", 1),
            (3, "minecraft:sugar", 1),
            (4, "minecraft:egg", 1),
            (5, "minecraft:sugar", 1),
            (6, "minecraft:wheat", 1),
            (7, "minecraft:wheat", 1),
            (8, "minecraft:wheat", 1),
        ],
    );
    a.set(POWER, "minecraft:redstone_block");
    let evs = a.ticks(4);
    assert_eq!(a.held(chest, "minecraft:cake"), 1, "cake not in the chest");
    assert_eq!(a.held(chest, "minecraft:bucket"), 3, "bucket remainders not in the chest");
    assert!(a.p.s.be_item_counts(CRAFTER).is_empty(), "grid not used up");
    assert!(!level_events(&evs).iter().any(|e| e.0 == 1049 || e.0 == 2010), "events for a stored craft");
    assert_eq!(a.dropped("minecraft:cake") + a.dropped("minecraft:bucket"), 0, "something thrown");
}

/// Into another crafter the result goes one item at a time: its slots fill evenly (one plank
/// in each of the first four), not one stack of four.
#[test]
fn crafter_feeds_another_crafter_one_item_at_a_time() {
    let Some(mut a) = open("crafter-chain") else { return };
    let next: P = [111, -59, 110];
    a.set(CRAFTER, "minecraft:crafter[orientation=east_up]");
    a.set(next, "minecraft:crafter[orientation=east_up]");
    a.put(CRAFTER, &[(4, "minecraft:oak_log", 1)]);
    a.set(POWER, "minecraft:redstone_block");
    let _ = a.ticks(4);
    let slots = a.p.s.slots(next);
    let counts: Vec<i32> = slots
        .iter()
        .map(|s| s.as_ref().filter(|it| it.get("id").and_then(Nbt::as_str) == Some("minecraft:oak_planks")))
        .map(|s| s.and_then(|it| crate::nbtio::int(it, "count")).unwrap_or(0) as i32)
        .collect();
    assert_eq!(counts, vec![1, 1, 1, 1, 0, 0, 0, 0, 0], "planks spread over the receiving crafter");
    assert_eq!(a.dropped("minecraft:oak_planks"), 0);
}

/// No recipe for the grid: the refusal event 1050, no `crafting` face, nothing used.
#[test]
fn crafter_without_recipe_reports_the_refusal() {
    let Some(mut a) = open("crafter-refused") else { return };
    a.set(CRAFTER, "minecraft:crafter[orientation=east_up]");
    a.put(CRAFTER, &[(0, "minecraft:stick", 1)]);
    a.set(POWER, "minecraft:redstone_block");
    let evs = a.ticks(4);
    assert!(level_events(&evs).contains(&(1050, 0)), "no refusal event");
    assert!(!crafting(&a));
    assert_eq!(a.held(CRAFTER, "minecraft:stick"), 1, "item used without a craft");
}

/// Placed by the player next to a powered block: `triggered` at once, and the crafter acts 4
/// ticks later (empty: the refusal event).
#[test]
fn crafter_placed_powered_acts_after_four_ticks() {
    let Some(mut a) = open("crafter-placed") else { return };
    let at: P = [112, -59, 110];
    a.set([113, -59, 110], "minecraft:redstone_block");
    a.p.run("player 112.5 -59 113.5 180 30\nhold minecraft:crafter\n");
    a.p.s.use_item_on_block([112, -60, 110], Dir::Up, [0.5, 1.0, 0.5]);
    assert!(a.name(at).starts_with("minecraft:crafter"), "crafter not placed: {}", a.name(at));
    assert!(a.name(at).contains("triggered=true"), "placed powered but not triggered");
    assert_eq!(a.p.s.be_get(at).and_then(|b| crate::nbtio::int(b, "triggered")), Some(1), "block entity signal");
    let evs = a.ticks(3);
    assert!(!level_events(&evs).iter().any(|e| e.0 == 1050), "acted before 4 ticks");
    let evs = a.ticks(1);
    assert!(level_events(&evs).contains(&(1050, 0)), "placed powered crafter did not act");
}

/// The countdown re-read from the block entity data (`/data`, as after a load) still ends
/// the `crafting` face.
#[test]
fn crafter_countdown_resumes_from_its_data() {
    let Some(mut a) = open("crafter-countdown") else { return };
    a.set(CRAFTER, "minecraft:crafter[orientation=east_up,crafting=true]");
    a.cmd("data merge block 110 -59 110 {crafting_ticks_remaining:3}");
    let _ = a.ticks(2);
    assert!(crafting(&a), "ended too early");
    let _ = a.ticks(1);
    assert!(!crafting(&a), "countdown read from the data did not run");
}

/// Cost of a crafter's craft (loaded machine figures in `docs/perf.md`): the recipe search of a
/// 3×3 grid, the remainders through a second search (`remaining`) against the recipe already
/// found (`remaining_of`), and a whole craft thrown out of the crafter.
/// `cargo test --release -p rc-sim --lib bench_crafter_craft -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_crafter_craft() {
    use crate::player::Stack;
    let Some(mut a) = open("crafter-bench") else { return };
    let log = || Some(Stack { id: "minecraft:oak_log".into(), count: 64, components: None });
    let cake: Vec<Option<Stack>> = ["milk_bucket", "milk_bucket", "milk_bucket", "sugar", "egg", "sugar", "wheat", "wheat", "wheat"]
        .iter()
        .map(|id| Some(Stack { id: format!("minecraft:{id}"), count: 1, components: None }))
        .collect();
    let planks: Vec<Option<Stack>> = (0..9).map(|i| if i == 4 { log() } else { None }).collect();
    let n = 2_000u32;
    let per = |t: std::time::Instant| t.elapsed().as_nanos() as f64 / f64::from(n) / 1000.0;
    for pass in 0..2 {
        for (name, grid) in [("planks", &planks), ("cake", &cake)] {
            let s = &a.p.s;
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(s.life.recipes.craft(&s.life.items, std::hint::black_box(grid), 3, &|_| None));
            }
            let search = per(t);
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(s.life.recipes.remaining(&s.life.items, std::hint::black_box(grid), &|_| None));
            }
            let searched = per(t);
            let i = s.life.recipes.craft(&s.life.items, grid, 3, &|_| None).map(|r| r.0).expect("recipe");
            let t = std::time::Instant::now();
            for _ in 0..n {
                std::hint::black_box(s.life.recipes.remaining_of(i, &s.life.items, std::hint::black_box(grid)));
            }
            let known = per(t);
            println!(
                "pass {pass}, {name}: search {search:.2} µs; remainders by a second search {searched:.2} µs, \
                 from the recipe found {known:.3} µs"
            );
        }
        // A whole craft of planks thrown out (scheduled tick, countdown, item spawned).
        a.set(CRAFTER, "minecraft:crafter[orientation=east_up]");
        a.put(CRAFTER, &[(4, "minecraft:oak_log", 64)]);
        let t = std::time::Instant::now();
        for _ in 0..60 {
            let s = a.p.s.get(CRAFTER);
            a.p.s.crafter_craft(s, CRAFTER);
        }
        println!("pass {pass}: whole craft {:.2} µs", t.elapsed().as_nanos() as f64 / 60.0 / 1000.0);
        a.p.s.life.drops.clear();
        let _ = a.p.take_events();
    }
}

/// Comparator: the number of slots that hold an item or are disabled, not a fill ratio.
#[test]
fn crafter_comparator_counts_used_slots() {
    let Some(mut a) = open("crafter-comparator") else { return };
    a.set(CRAFTER, "minecraft:crafter[orientation=east_up]");
    a.put(CRAFTER, &[(0, "minecraft:stone", 64), (3, "minecraft:stick", 1), (8, "minecraft:dirt", 5)]);
    if let Some(be) = a.p.s.be_get_mut(CRAFTER) {
        crate::nbtio::set(be, "disabled_slots", Nbt::IntArray(vec![1]));
    }
    let s = a.p.s.get(CRAFTER);
    assert_eq!(a.p.s.analog_output(s, CRAFTER, Dir::East), 4, "3 items + 1 disabled slot");
}
