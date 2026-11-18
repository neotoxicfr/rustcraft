//! Micro-benches of the mob network sync (`docs/perf.md`), each `#[ignore]`d: the crowd of
//! `tracking_stream_tests` on the flat test world, warmed up 100 ticks.
//!
//! `cargo test --release -p rc-sim bench_mob_network_sync -- --ignored --nocapture --test-threads=1`
//!
//! Never beside the proofs (`--include-ignored`): a bench turns the cross-checks of the test
//! runs off for the whole process, which is refused there, and it would then measure them too.
//!
//! The switches are read once: one process per variant of the synced data refresh
//! (`RUSTCRAFT_OPT_SYNCED_SLOT_DIRECT=0`, `RUSTCRAFT_OPT_SYNCED_WRITE_INLINED=0`,
//! `RUSTCRAFT_OPT_SYNCED_NAME_IN_PLACE=0`, `RUSTCRAFT_OPT_UNSENT_CHANGES_UNCOPIED=0`). The
//! attribute read and the refresh ahead of the tracking are compared inside one process, the
//! variants alternated.

use std::time::Instant;

use super::body_movement_tests::{COURSE, cmd};
use super::mob::{Attr, Mob, cross_checks};
use super::track::{ForcedRefresh, force_refresh};
use super::tracking_stream_tests::{Area, COURSE_AREA, send_loaded_chunks, spawn_crowd};
use crate::bench_kit::{median, report, run_ticks};
use crate::blocks::probe::Probe;

const BENCH: &str = "mob_network_sync";

/// Every block of the chunks the flat test world loads but a margin of two blocks.
const LOADED_AREA: Area = [-30.0, 158.0, -30.0, 46.0];

/// The crowd of `mobs` mobs on `area`, tracked, after 100 ticks; the comparisons of the test
/// runs are off.
fn warmed_crowd(tag: &str, mobs: usize, area: Area) -> (Probe, std::path::PathBuf) {
    cross_checks::turn_off_for_a_bench();
    super::index::set_verify(false);
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    for c in COURSE {
        cmd(&mut p, c);
    }
    spawn_crowd(&mut p, mobs, area);
    send_loaded_chunks(&mut p);
    run_ticks(&mut p, 100);
    (p, dir)
}

fn percentile(values: &[f64], percent: usize) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.get((sorted.len() * percent / 100).min(sorted.len().saturating_sub(1))).copied().unwrap_or(f64::NAN)
}

/// `passes` timings of `work` over every mob of the crowd, in ns per mob.
fn ns_per_mob(
    p: &mut Probe,
    passes: usize,
    mut prepare: impl FnMut(&mut Mob),
    mut work: impl FnMut(&mut Mob),
) -> Vec<f64> {
    let mobs = p.s.mobs.list.iter().count().max(1);
    (0..passes)
        .map(|_| {
            p.s.mobs.list.iter_mut().for_each(&mut prepare);
            let start = Instant::now();
            p.s.mobs.list.iter_mut().for_each(&mut work);
            start.elapsed().as_nanos() as f64 / mobs as f64
        })
        .collect()
}

/// Refresh of the synced data of a crowd of 1,000 (one mob in 25 named, then all of them),
/// and the changes of an unseen mob dropped (two values changed per mob).
#[test]
#[ignore = "micro-bench"]
fn bench_mob_network_sync_refresh() {
    let (mut p, _dir) = warmed_crowd("bench-synced-refresh", 1000, COURSE_AREA);
    let values: usize = p.s.mobs.list.iter().map(|m| m.data.len()).sum();
    let mobs = p.s.mobs.list.iter().count();
    report(BENCH, "refresh", "synced_values_per_mob", values as f64 / mobs as f64);
    for round in 0..3 {
        let refresh = ns_per_mob(&mut p, 400, |_| {}, Mob::sync_data);
        report(BENCH, &format!("refresh_round_{round}"), "ns_per_mob", median(&refresh));
    }
    let unseen = ns_per_mob(
        &mut p,
        400,
        |m| {
            // Two values change (air, freezing), as for a mob drowning out of sight.
            m.air ^= 1;
            m.ticks_frozen ^= 1;
            m.sync_data();
        },
        Mob::discard_changes,
    );
    report(BENCH, "unseen_changes_dropped", "ns_per_mob", median(&unseen));
    p.s.mobs.list.iter_mut().for_each(|m| {
        m.custom_name = Some(rc_proto::nbt::Nbt::String(format!("Mob number {}", m.id)));
    });
    let named = ns_per_mob(&mut p, 400, |_| {}, Mob::sync_data);
    report(BENCH, "refresh_all_named", "ns_per_mob", median(&named));
}

/// The attributes a mob's movement reads every tick.
fn movement_attributes(m: &Mob) -> [&Attr; 9] {
    let a = &m.attrs;
    [
        &a.gravity,
        &a.step,
        &a.safe_fall,
        &a.fall_mult,
        &a.friction_mod,
        &a.air_drag_mod,
        &a.speed,
        &a.jump,
        &a.max_health,
    ]
}

/// ns per read of `read` over `attributes`, 200 times; `read` is compiled into the loop, as a
/// caller of the game compiles its own read.
fn ns_per_read(attributes: &[&Attr], read: impl Fn(&Attr) -> f64) -> f64 {
    let start = Instant::now();
    let mut sum = 0.0;
    for _ in 0..200 {
        for a in attributes {
            sum += read(std::hint::black_box(a));
        }
    }
    std::hint::black_box(sum);
    start.elapsed().as_nanos() as f64 / (200 * attributes.len()) as f64
}

/// Read of the attributes of the movement that carry no modifier, over the crowd: the shortcut
/// against the modifier passes, alternated, and `value` as the switch sets it.
#[test]
#[ignore = "micro-bench"]
fn bench_mob_network_sync_attribute_value() {
    let (p, _dir) = warmed_crowd("bench-attribute-value", 1000, COURSE_AREA);
    let all: Vec<&Attr> = p.s.mobs.list.iter().flat_map(movement_attributes).collect();
    let attributes: Vec<&Attr> = all.iter().copied().filter(|a| a.mods.is_empty()).collect();
    report(BENCH, "attribute", "share_without_modifier", attributes.len() as f64 / all.len() as f64);
    let (mut shortcut, mut passes, mut as_set) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..60 {
        shortcut.push(ns_per_read(&attributes, Attr::value_without_modifier));
        passes.push(ns_per_read(&attributes, Attr::value_through_modifiers));
        as_set.push(ns_per_read(&attributes, Attr::value));
    }
    for (case, ns) in
        [("attribute_shortcut", &shortcut), ("attribute_modifier_passes", &passes), ("attribute_value_as_set", &as_set)]
    {
        report(BENCH, case, "ns_per_read", median(ns));
        report(BENCH, case, "ns_per_read_best", percentile(ns, 0));
    }
}

/// One tracking pass over the whole crowd after each tick: the synced data refreshed by the
/// tracking, or ahead of it in 1 to 32 runs, alternated inside each tick (µs per pass).
fn tracking_pass_by_runs(case: &str, mobs: usize, area: Area, ticks: usize) {
    let (mut p, _dir) = warmed_crowd(case, mobs, area);
    let variants: Vec<(String, ForcedRefresh)> =
        std::iter::once(("by_the_tracking".to_owned(), ForcedRefresh::InTracking))
            .chain([1, 2, 4, 8, 16, 32].map(|runs| (format!("ahead_{runs}_runs"), ForcedRefresh::Ahead { runs })))
            .collect();
    let mut us_per_pass = vec![Vec::new(); variants.len()];
    for tick in 0..ticks {
        run_ticks(&mut p, 1);
        for turn in 0..variants.len() {
            let variant = (turn + tick) % variants.len();
            force_refresh(Some(variants[variant].1));
            let start = Instant::now();
            p.s.track_mobs();
            us_per_pass[variant].push(start.elapsed().as_secs_f64() * 1e6);
            let _ = p.take_events();
        }
        force_refresh(None);
    }
    report(BENCH, case, "mobs", p.s.mobs.list.iter().count() as f64);
    // On a loaded machine the low decile says what a pass costs when nothing preempts it.
    for ((name, _), us) in variants.iter().zip(&us_per_pass) {
        report(BENCH, &format!("{case}_{name}"), "us_per_pass_low_decile", percentile(us, 10));
        report(BENCH, &format!("{case}_{name}"), "us_per_pass", median(us));
        report(BENCH, &format!("{case}_{name}"), "us_per_pass_p99", percentile(us, 99));
    }
}

/// Crowds named by the mobs spawned (some die during the warm-up: the count left is reported).
#[test]
#[ignore = "micro-bench"]
fn bench_mob_network_sync_tracking_pass() {
    eprintln!("[bench] {BENCH} logical cores {}", crate::helper_pool::logical_cores());
    tracking_pass_by_runs("tracking_250", 250, COURSE_AREA, 300);
    tracking_pass_by_runs("tracking_500", 500, COURSE_AREA, 300);
    tracking_pass_by_runs("tracking_1000", 1000, COURSE_AREA, 300);
    tracking_pass_by_runs("tracking_5000", 5000, LOADED_AREA, 120);
}

/// Crowds around the count where the helper threads take over (1,024 mobs): where the server
/// thread alone starts losing against the refresh by the tracking, and where the helpers win.
#[test]
#[ignore = "micro-bench"]
fn bench_mob_network_sync_tracking_pass_near_the_helper_threshold() {
    eprintln!("[bench] {BENCH} logical cores {}", crate::helper_pool::logical_cores());
    tracking_pass_by_runs("tracking_1500", 1500, LOADED_AREA, 200);
    tracking_pass_by_runs("tracking_2000", 2000, LOADED_AREA, 200);
    tracking_pass_by_runs("tracking_3000", 3000, LOADED_AREA, 160);
}

/// Heap of each mob of a crowd (what a copy of the mob asks the allocator for), with the share
/// of its synced values, at 1,000 and 5,000 mobs.
#[test]
#[ignore = "measurement"]
fn measure_mob_network_sync_heap_of_each_mob() {
    use crate::test_alloc::requested_bytes;
    for (mobs, area) in [(1000, COURSE_AREA), (5000, LOADED_AREA)] {
        let (p, _dir) = warmed_crowd("measure-mob-heap", mobs, area);
        let (mut heap, mut synced, mut count) = (0u64, 0usize, 0usize);
        for m in p.s.mobs.list.iter() {
            let before = requested_bytes();
            let copy = m.clone();
            heap += requested_bytes() - before;
            synced += copy.data.len() * std::mem::size_of::<super::mob::Synced>();
            count += 1;
        }
        let case = format!("heap_{mobs}");
        report(BENCH, &case, "mobs", count as f64);
        report(BENCH, &case, "struct_bytes_per_mob", std::mem::size_of::<Mob>() as f64);
        report(BENCH, &case, "heap_bytes_per_mob", heap as f64 / count as f64);
        report(BENCH, &case, "synced_values_bytes_per_mob", synced as f64 / count as f64);
    }
}
