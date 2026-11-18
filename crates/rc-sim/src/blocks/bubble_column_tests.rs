//! Bubble columns while the game runs: built 20 ticks after soul sand goes under a water
//! shaft, turned by a magma block, grown into new water above, undone without support; their
//! push on the mobs and minecarts the server moves, and the splashes over an open top. Before,
//! the water stayed water and only the player's own client was pushed.

use super::P;
use super::test_area::{Area, open};
use crate::inside_blocks::Who;

/// Water shaft at x = z = `col` from y = -59 up to `top` inside glass walls; floor (-60) of glass.
fn shaft(a: &mut Area, col: i32, top: i32) {
    a.p.run(&format!(
        "fill {} -60 {} {} {} {} minecraft:glass\nfill {col} -59 {col} {col} {top} {col} minecraft:water\n",
        col - 1,
        col - 1,
        col + 1,
        top + 1,
        col + 1
    ));
}

fn cells(a: &Area, col: i32, ys: std::ops::RangeInclusive<i32>) -> Vec<String> {
    ys.map(|y| a.name([col, y, col])).collect()
}

fn all(n: usize, state: &str) -> Vec<String> {
    vec![state.to_owned(); n]
}

const UP: &str = "minecraft:bubble_column[drag=false]";
const DOWN: &str = "minecraft:bubble_column[drag=true]";
const WATER: &str = "minecraft:water[level=0]";

#[test]
fn bubble_column_forms_turns_grows_and_undoes() {
    let Some(mut a) = open("bubble-column") else { return };
    let col = 110;
    shaft(&mut a, col, -55);
    let _ = a.ticks(10);
    assert_eq!(cells(&a, col, -59..=-55), all(5, WATER));
    let floor: P = [col, -60, col];

    a.set(floor, "minecraft:soul_sand");
    let _ = a.ticks(19);
    assert_eq!(a.name([col, -59, col]), WATER, "column built before the water's 20-tick tick");
    let _ = a.ticks(1);
    assert_eq!(cells(&a, col, -59..=-55), all(5, UP), "upward column over soul sand");

    a.set(floor, "minecraft:magma_block");
    let _ = a.ticks(4);
    assert_eq!(a.name([col, -59, col]), UP, "turned before the 5-tick check");
    let _ = a.ticks(1);
    assert_eq!(cells(&a, col, -59..=-55), all(5, DOWN), "downward column over magma");

    a.set([col, -54, col], "minecraft:water");
    let _ = a.ticks(5);
    assert_eq!(a.name([col, -54, col]), DOWN, "column not grown into the water added above");

    a.set(floor, "minecraft:stone");
    let _ = a.ticks(5);
    assert_eq!(cells(&a, col, -59..=-54), all(6, WATER), "column kept without support");
}

/// A mob (zombie, which sinks in water) and a minecart at the bottom of upward columns rise.
#[test]
fn bubble_column_lifts_mobs_and_minecarts() {
    let Some(mut a) = open("bubble-column-push") else { return };
    for col in [110, 114] {
        shaft(&mut a, col, -52);
        a.set([col, -60, col], "minecraft:soul_sand");
    }
    let _ = a.ticks(20);
    assert_eq!(a.name([110, -52, 110]), UP, "columns not built");
    a.cmd("summon minecraft:zombie 110.5 -59 110.5 {PersistenceRequired:1b}");
    a.cmd("summon minecraft:minecart 114.5 -59 114.5");
    let zombie = a.p.s.mobs.list.iter().find(|m| m.kind == crate::mobs::species::Kind::Zombie).map(|m| m.id);
    let zombie = zombie.expect("zombie summoned");
    let cart = a.p.s.carts.list.iter().flatten().next().map(|c| c.id).expect("minecart summoned");
    let _ = a.game_ticks(20);
    let zy = a.p.s.mobs.list.iter().find(|m| m.id == zombie).map(|m| m.pos()[1]).expect("zombie alive");
    let cy = a.p.s.carts.list.iter().flatten().find(|c| c.id == cart).map(|c| c.pos[1]).expect("minecart");
    // Both read before asserting: a failure shows where each one stands.
    assert!(zy > -58.0 && cy > -58.0, "not lifted by the column: zombie y = {zy}, minecart y = {cy}");
}

/// A ground item carried to a column's open top makes the server send splashes there (the
/// crossed-block pass no longer skips an item near a column).
#[test]
fn bubble_column_splashes_over_a_lifted_item() {
    let Some(mut a) = open("bubble-column-item") else { return };
    let col = 118;
    shaft(&mut a, col, -57);
    a.set([col, -60, col], "minecraft:soul_sand");
    a.set([col, -56, col], "minecraft:air");
    let _ = a.ticks(20);
    assert_eq!(a.name([col, -57, col]), UP);
    a.cmd("summon minecraft:item 118.5 -59 118.5 {Item:{id:\"minecraft:stone\",count:1},PickupDelay:32767}");
    let evs = a.game_ticks(40);
    assert!(a.particles(&evs, "minecraft:splash") > 0, "no splash over the item at the column top");
}

/// Cost of the column check added to every update of a liquid block (loaded machine figures in
/// `docs/perf.md`): flowing water (the common case), still source water over stone, and the
/// whole neighbour reaction of the source for scale.
/// `cargo test --release -p rc-sim --lib bench_water_column_check -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_water_column_check() {
    let Some(mut a) = open("bubble-column-bench") else { return };
    a.p.run("fill 120 -59 120 120 -59 120 minecraft:water\nsetblock 122 -59 120 minecraft:water[level=3]\n");
    let n = 2_000_000u32;
    let time = |a: &mut Area, p: P, whole: bool| {
        let s = a.p.s.get(p);
        let t = std::time::Instant::now();
        for _ in 0..n {
            if whole {
                a.p.s.liquid_neighbor_changed(std::hint::black_box(s), p);
            } else {
                a.p.s.water_plan_column(std::hint::black_box(s), p);
            }
        }
        t.elapsed().as_nanos() as f64 / f64::from(n)
    };
    for pass in 0..2 {
        let flowing = time(&mut a, [122, -59, 120], false);
        let source = time(&mut a, [120, -59, 120], false);
        let whole = time(&mut a, [120, -59, 120], true);
        println!(
            "pass {pass}: column check {flowing:.1} ns (flowing), {source:.1} ns (source over stone); \
             whole neighbour reaction {whole:.1} ns"
        );
    }
}

/// The player standing in a column's open top cell: two splashes and two bubbles sent; none
/// with a block over the column.
#[test]
fn bubble_column_splashes_over_an_open_top() {
    let Some(mut a) = open("bubble-column-splash") else { return };
    let col = 110;
    shaft(&mut a, col, -56);
    a.set([col, -60, col], "minecraft:soul_sand");
    a.set([col, -55, col], "minecraft:air");
    let _ = a.ticks(20);
    assert_eq!(a.name([col, -56, col]), UP);
    a.p.run(&format!("player {col}.5 -56 {col}.5 0 0\n"));
    a.p.s.inside_forget_player();
    let _ = a.p.take_events();
    let _ = a.p.s.inside_blocks(&mut Who::Player);
    let evs = a.p.take_events();
    assert_eq!(a.particles(&evs, "minecraft:splash"), 2, "splashes over the open top");
    assert_eq!(a.particles(&evs, "minecraft:bubble"), 2, "bubbles over the open top");

    a.set([col, -55, col], "minecraft:glass");
    a.p.s.inside_forget_player();
    let _ = a.p.take_events();
    let _ = a.p.s.inside_blocks(&mut Who::Player);
    let evs = a.p.take_events();
    assert_eq!(a.particles(&evs, "minecraft:splash"), 0, "splashes under a block");
}
