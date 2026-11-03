//! Micro-bench of the mob path search on the village of the bench model world: the 2,000 drawn
//! searches of the equivalence proof (ground walkers, flyers, amphibians, swimmers), replayed
//! with each lead of `SearchOptions` alone, then all together. Readings per search: time (µs),
//! allocations (`memstats` feature) and block path types computed (deterministic count).
//!
//! `cargo test --release -p rc-sim [--features memstats] bench_path_search -- --ignored --nocapture --test-threads=1`
//! with `RUSTCRAFT_OPT_WORLD=<model world>`. One thread only: the server is not shared between
//! threads (its sky darkening cache is a `Cell`) and the searches read the live world on the tick
//! thread.

use crate::bench_kit::{allocations, median, report};
use crate::mobs::nav::{MemoKind, SearchOptions, TypeRules, block_type_computations, search_tests};

fn cases() -> Vec<(&'static str, SearchOptions)> {
    let old = SearchOptions::OLD;
    vec![
        ("old", old),
        ("state_once", SearchOptions { type_rules: TypeRules::StateOnce, ..old }),
        ("type_table", SearchOptions { type_rules: TypeRules::Table, ..old }),
        ("memo_map", SearchOptions { memo: MemoKind::HashMap, ..old }),
        ("memo_slots", SearchOptions { memo: MemoKind::DirectMapped, ..old }),
        ("scratch", SearchOptions { scratch: true, ..old }),
        ("region", SearchOptions { region: true, ..old }),
        ("palette", SearchOptions { palette: true, ..old }),
        ("table_memo", SearchOptions { type_rules: TypeRules::Table, memo: MemoKind::DirectMapped, ..old }),
        ("all_with_memo_map", SearchOptions { memo: MemoKind::HashMap, ..SearchOptions::ALL_NEW }),
        ("table_region", SearchOptions { type_rules: TypeRules::Table, region: true, ..old }),
        ("all_but_memo", SearchOptions { memo: MemoKind::Off, ..SearchOptions::ALL_NEW }),
        ("all_but_palette", SearchOptions { palette: false, ..SearchOptions::ALL_NEW }),
        ("all_new", SearchOptions::ALL_NEW),
    ]
}

/// Position key and slot of a direct-mapped cache of 4,096 entries like the game's level-wide
/// path type cache (packed position, 64-bit mix): only the cost of a hit is measured.
fn cache_slot(p: [i32; 3]) -> (u64, usize) {
    let key = ((p[0] as i64 & 0x3FF_FFFF) << 38 | (p[2] as i64 & 0x3FF_FFFF) << 12 | (p[1] as i64 & 0xFFF)) as u64;
    let mut h = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 32;
    h ^= h >> 16;
    (key, (h & 4095) as usize)
}

/// Block path type of one cell: each way of finding it, and a hit of a level-wide cache (the lead
/// left out: its hit must beat the table read to gain anything).
#[test]
#[ignore = "micro-bench"]
fn bench_block_type_lookup() {
    let Some(mut s) = search_tests::village_server("path-bench-types") else { return };
    let view = search_tests::draw_searches(&s, 1).remove(0).view;
    let mut r = crate::mobs::jrand::JRand::new(5);
    let c = view.block;
    let cells: Vec<[i32; 3]> = (0..1_000_000)
        .map(|_| [c[0] + r.next_int_bound(97) - 48, c[1] + r.next_int_bound(17) - 8, c[2] + r.next_int_bound(97) - 48])
        .collect();
    let old = SearchOptions::OLD;
    let cases = [
        ("by_name", old),
        ("state_once", SearchOptions { type_rules: TypeRules::StateOnce, ..old }),
        ("table", SearchOptions { type_rules: TypeRules::Table, ..old }),
        ("table_region", SearchOptions { type_rules: TypeRules::Table, region: true, ..old }),
    ];
    let mut cache = vec![(u64::MAX, 0u8); 4096];
    for &p in &cells[..4096] {
        let (key, slot) = cache_slot(p);
        cache[slot] = (key, 1);
    }
    for pass in 0..3 {
        // Built once at server opening, on the opening thread.
        let t0 = std::time::Instant::now();
        let rules = crate::mobs::nav::path_states::PathRules::new(&s.data, &s.bs.t.fluid_mob, |st| s.land_ok(st, true));
        report("block_type", &format!("table_build#{pass}"), "ms", t0.elapsed().as_secs_f64() * 1e3);
        std::hint::black_box(rules);
        for (name, opts) in cases {
            report(
                "block_type",
                &format!("{name}#{pass}"),
                "ns_per_cell",
                search_tests::block_type_ns(&s, &view, &cells, opts),
            );
        }
        let t0 = std::time::Instant::now();
        let mut hits = 0u32;
        for &p in &cells {
            let (key, slot) = cache_slot(p);
            hits += u32::from(cache[slot].0 == key) + u32::from(cache[slot].1);
        }
        std::hint::black_box(hits);
        report(
            "block_type",
            &format!("level_cache_hit#{pass}"),
            "ns_per_cell",
            t0.elapsed().as_nanos() as f64 / cells.len() as f64,
        );
    }
    s.stop_parked();
}

/// Three timed runs of `searches` with `opts`: median µs, allocations and block types per search.
fn measure(s: &crate::server::Server, searches: &[&search_tests::Search], opts: SearchOptions) -> (f64, f64, f64) {
    let n = searches.len() as f64;
    let mut runs = Vec::new();
    let (mut allocs, mut computed) = (0, 0);
    for _ in 0..3 {
        let (a0, c0, t0) = (allocations(), block_type_computations(), std::time::Instant::now());
        for q in searches {
            std::hint::black_box(search_tests::run_search_with(s, q, opts));
        }
        runs.push(t0.elapsed().as_secs_f64() * 1e6 / n);
        (allocs, computed) = (allocations() - a0, block_type_computations() - c0);
    }
    (median(&runs), allocs as f64 / n, computed as f64 / n)
}

#[test]
#[ignore = "micro-bench"]
fn bench_path_search() {
    let Some(mut s) = search_tests::village_server("path-bench") else { return };
    let searches = search_tests::draw_searches(&s, 2000);
    let all: Vec<&search_tests::Search> = searches.iter().collect();
    // The village's own load: villagers only (two profiles of twenty: with and without doors).
    let villagers: Vec<&search_tests::Search> =
        searches.iter().filter(|q| q.view.kind == crate::mobs::species::Kind::Villager).collect();
    for pass in 0..3 {
        for (set, list) in [("all", &all), ("villagers", &villagers)] {
            for (name, opts) in cases() {
                let (us, allocs, computed) = measure(&s, list, opts);
                let case = format!("{set}/{name}#{pass}");
                report("path_search", &case, "us_per_search", us);
                report("path_search", &case, "block_types_per_search", computed);
                if cfg!(feature = "memstats") {
                    report("path_search", &case, "allocs_per_search", allocs);
                }
            }
        }
    }
    s.stop_parked();
}
