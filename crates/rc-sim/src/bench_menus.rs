//! Menus micro-bench (`docs/perf.md`, menus), on a flat world with the player's 36 slots full
//! (half of them damaged swords, which carry a patch):
//! - "broadcast": an open chest of 27 full stacks (63 menu slots) and nothing changing: one
//!   `broadcast_menus` (re-read of the chest, difference broadcast, flush) as each tick does;
//! - "pull-double": an open double chest of 54 stacks, one `pull_menu`;
//! - "clicks": a cycle of four clicks on an open chest (take and put back, shift-click there and
//!   back) through the click packet's handler.
//!
//! Each with every switch off (the former code), each lead alone, then all on: nanoseconds and
//! allocations (with `memstats`) per call. Run alone: `cargo test --release -p rc-sim
//! [--features memstats] bench_menus -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use crate::bench_kit::{allocations, cmd, median, report};
use crate::blocks::probe::Probe;
use crate::menus::{FORCED_SWITCHES, MenuSwitches, mode};
use crate::player::Stack;
use crate::tests_g3f13::{finish, open};

const CHEST: [i32; 3] = [2, -60, 0];
const DOUBLE_RIGHT: [i32; 3] = [6, -60, 0];

/// The former code, each lead alone, everything on.
fn cases() -> Vec<(&'static str, MenuSwitches)> {
    let off = MenuSwitches::ALL_OFF;
    vec![
        ("off", off),
        ("broadcast-in-place", MenuSwitches { broadcast_in_place: true, ..off }),
        ("in-place-and-reuse", MenuSwitches { broadcast_in_place: true, remote_copy_reuse: true, ..off }),
        ("pull-without-decoding", MenuSwitches { pull_without_decoding: true, ..off }),
        ("fewer-allocations", MenuSwitches { fewer_allocations: true, ..off }),
        ("all-on", MenuSwitches::ALL_ON),
    ]
}

/// The player's bag and hotbar full: stone stacks and damaged swords in turn.
fn fill_player(p: &mut Probe) {
    for (k, slot) in p.s.slots[9..45].iter_mut().enumerate() {
        *slot = Some(if k % 2 == 0 {
            Stack { id: "minecraft:stone".into(), count: 64, components: None }
        } else {
            let damage = rc_proto::nbt::Nbt::Int(k as i32 + 1);
            let patch = rc_proto::nbt::Nbt::Compound(vec![("minecraft:damage".into(), damage)]);
            Stack { id: "minecraft:iron_sword".into(), count: 1, components: Some(patch) }
        });
    }
}

fn fill_container(p: &mut Probe, at: [i32; 3], item: &str) {
    for k in 0..27 {
        cmd(p, &format!("item replace block {} {} {} container.{k} with minecraft:{item}", at[0], at[1], at[2]));
    }
}

/// Median ns and allocations per call of `f` over 5 runs of `n` calls.
fn time_calls(p: &mut Probe, n: u32, mut f: impl FnMut(&mut Probe)) -> (f64, f64) {
    let (mut ns, mut allocs) = (Vec::new(), Vec::new());
    for _ in 0..5 {
        let (start, before) = (Instant::now(), allocations());
        for _ in 0..n {
            f(p);
        }
        ns.push(start.elapsed().as_nanos() as f64 / f64::from(n));
        allocs.push((allocations() - before) as f64 / f64::from(n));
        let _ = p.take_events();
    }
    (median(&ns), median(&allocs))
}

fn report_case(case: &str, label: &str, (ns, allocs): (f64, f64)) {
    report("menus", &format!("{case}-{label}"), "ns_per_call", ns);
    if cfg!(feature = "memstats") {
        report("menus", &format!("{case}-{label}"), "allocs_per_call", allocs);
    }
}

fn broadcast(label: &str, switches: MenuSwitches) {
    FORCED_SWITCHES.with(|f| f.set(Some(switches)));
    let (mut p, dir) = open(&format!("bench-menus-broadcast-{label}"));
    cmd(&mut p, "setblock 2 -60 0 minecraft:chest");
    fill_container(&mut p, CHEST, "cobblestone 64");
    fill_player(&mut p);
    assert!(p.s.use_block(CHEST), "chest menu");
    p.s.broadcast_menus();
    report_case("broadcast", label, time_calls(&mut p, 20_000, |p| p.s.broadcast_menus()));
    finish(p, dir);
}

fn pull_double(label: &str, switches: MenuSwitches) {
    FORCED_SWITCHES.with(|f| f.set(Some(switches)));
    let (mut p, dir) = open(&format!("bench-menus-pull-{label}"));
    cmd(&mut p, "setblock 5 -60 0 minecraft:chest[facing=north,type=left]");
    cmd(&mut p, "setblock 6 -60 0 minecraft:chest[facing=north,type=right]");
    fill_container(&mut p, DOUBLE_RIGHT, "cobblestone 64");
    fill_container(&mut p, [5, -60, 0], "iron_ingot 32");
    assert!(p.s.use_block(DOUBLE_RIGHT), "double chest menu");
    p.s.broadcast_menus();
    report_case("pull-double", label, time_calls(&mut p, 20_000, |p| p.s.pull_menu()));
    finish(p, dir);
}

/// Take and put back chest slot 0, shift-click it into the bag (slot 27 is free) and back.
fn click_cycle(p: &mut Probe) {
    for (slot, m) in [(0, mode::PICKUP), (0, mode::PICKUP), (0, mode::QUICK_MOVE), (27, mode::QUICK_MOVE)] {
        let (window, state) = p.s.life.menu.as_ref().map(|m| (m.window, m.state)).expect("menu");
        p.s.on_click(window, state, slot, 0, m, Vec::new(), None);
    }
}

fn clicks(label: &str, switches: MenuSwitches) {
    FORCED_SWITCHES.with(|f| f.set(Some(switches)));
    let (mut p, dir) = open(&format!("bench-menus-clicks-{label}"));
    cmd(&mut p, "setblock 2 -60 0 minecraft:chest");
    cmd(&mut p, "item replace block 2 -60 0 container.0 with minecraft:cobblestone 64");
    fill_player(&mut p);
    p.s.slots[9] = None;
    assert!(p.s.use_block(CHEST), "chest menu");
    p.s.broadcast_menus();
    let (ns, allocs) = time_calls(&mut p, 2_000, click_cycle);
    report_case("clicks", label, (ns / 4.0, allocs / 4.0));
    finish(p, dir);
}

#[test]
#[ignore]
fn bench_menus() {
    for (label, switches) in cases() {
        broadcast(label, switches);
        pull_double(label, switches);
        clicks(label, switches);
    }
    FORCED_SWITCHES.with(|f| f.set(None));
}
