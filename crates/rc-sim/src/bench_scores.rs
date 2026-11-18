//! Micro-bench of the scoreboard switches (`RUSTCRAFT_OPT_SCOREHOLDERMAP`, `SCOREONELOOKUP`,
//! `TEAMMEMBERINDEX`, `SCOREUUIDSET`), on the board alone (no world: neither the selector nor the
//! command parsing is measured): a command writing every holder (`players add`, `operation`), the
//! save, the start of tracking, entity removals while one fake player holds a score, team lookups
//! and decorated names, joins alternating with push lookups. One thread: the scoreboard lives on
//! the server thread (no multi-core or GPU variant). Run alone:
//! `cargo test --release -p rc-sim [--features memstats] bench_scores -- --ignored --nocapture --test-threads=1`.

use std::hint::black_box;
use std::time::Instant;

use rc_proto::nbt::Nbt;
use rc_text::Component;

use crate::bench_kit;
use crate::commands::scores::{Board, BoardPaths, FORCED_PATHS, Objective, Team};
use crate::nbtio::uuid_text;

const RUNS: usize = 5;

fn uuids(n: usize, salt: u64) -> Vec<String> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ salt;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    (0..n).map(|_| uuid_text(u128::from(next()) << 64 | u128::from(next()))).collect()
}

fn objective(name: &str) -> Objective {
    Objective {
        name: name.into(),
        criteria: "dummy".into(),
        display: Nbt::String(name.into()),
        render: "integer".into(),
        auto_update: false,
        format: None,
    }
}

fn team(name: &str, players: Vec<String>) -> Team {
    Team {
        name: name.into(),
        display: Nbt::String(name.into()),
        color: Some("red".into()),
        friendly_fire: true,
        see_invisibles: true,
        prefix: Nbt::String("[".into()),
        suffix: Nbt::String("]".into()),
        nametag: "always".into(),
        death: "always".into(),
        collision: "pushOwnTeam".into(),
        players,
    }
}

/// Board on `paths` with objective `o` displayed in the sidebar and a score for every holder.
fn board_with_scores(paths: BoardPaths, holders: &[String]) -> Board {
    let mut b = Board::with_paths(paths);
    b.add_objective(objective("o"));
    b.set_display_slot(1, Some("o"));
    for h in holders {
        b.set(h, "o", 1);
    }
    b.set("#c", "o", 3);
    b.out.clear();
    b
}

/// `scoreboard players add <holders> o 1`, as the server runs it on each path.
fn players_add(b: &mut Board, holders: &[String]) {
    let target = b.score_target("o");
    for h in holders {
        if b.paths().one_lookup {
            b.update_score(h, "o", target, None, |v| v.wrapping_add(1));
        } else {
            let nv = b.get(h, "o").unwrap_or(0).wrapping_add(1);
            let sync = b.touch(h, "o");
            b.write(h, "o", nv, None, sync);
        }
    }
    b.out.clear();
}

/// `scoreboard players operation <holders> o += #c o`, as the server runs it on each path.
fn operation_add(b: &mut Board, holders: &[String]) {
    let target = b.score_target("o");
    for h in holders {
        if b.paths().one_lookup {
            let (_, sync) = b.open_score(h, "o");
            let (y, _) = b.open_score("#c", "o");
            let x = b.get(h, "o").unwrap_or(0);
            b.write_score(h, "o", target, x.wrapping_add(y), None, sync);
        } else {
            let sync = b.touch(h, "o");
            b.touch("#c", "o");
            let (x, y) = (b.get(h, "o").unwrap_or(0), b.get("#c", "o").unwrap_or(0));
            b.write(h, "o", x.wrapping_add(y), None, sync);
        }
    }
    b.out.clear();
}

/// Median nanoseconds and allocations (with `memstats`) per operation over `RUNS` runs of
/// `step`, each doing `ops` operations.
fn measure(ops: usize, mut step: impl FnMut()) -> (f64, f64) {
    let mut ns = Vec::new();
    let mut allocs = Vec::new();
    for _ in 0..RUNS {
        let (start, before) = (Instant::now(), bench_kit::allocations());
        step();
        ns.push(start.elapsed().as_nanos() as f64 / ops as f64);
        allocs.push((bench_kit::allocations() - before) as f64 / ops as f64);
    }
    (bench_kit::median(&ns), bench_kit::median(&allocs))
}

fn report(case: &str, label: &str, (ns, allocs): (f64, f64)) {
    bench_kit::report("scores", &format!("{case}_{label}"), "ns_per_op", ns);
    if cfg!(feature = "memstats") {
        bench_kit::report("scores", &format!("{case}_{label}"), "allocs_per_op", allocs);
    }
}

fn holder_cases(label: &str, paths: BoardPaths, n: usize) {
    let holders = uuids(n, n as u64);
    let mut b = board_with_scores(paths, &holders);
    report(&format!("players_add_{n}"), label, measure(n, || players_add(&mut b, &holders)));
    report(&format!("operation_{n}"), label, measure(n, || operation_add(&mut b, &holders)));
    report(&format!("save_{n}"), label, measure(n, || drop(black_box(b.to_nbt()))));
    // A temporary fake player written then reset (a common data pack pattern): the holder is
    // created and forgotten each time, in the listing order as well.
    report(
        &format!("temporary_holder_{n}"),
        label,
        measure(1000, || {
            for _ in 0..1000 {
                b.set("#tmp", "o", 1);
                b.reset("#tmp", None);
            }
            b.out.clear();
        }),
    );
    report(
        &format!("start_tracking_{n}"),
        label,
        measure(n, || {
            b.set_display_slot(1, None);
            b.set_display_slot(1, Some("o"));
            b.out.clear();
        }),
    );
}

/// 1,000 entities removed while a single fake player holds a score (none of them has one).
fn removal_case(label: &str, paths: BoardPaths) {
    let removed: Vec<u128> = uuids(1000, 7).iter().map(|u| crate::nbtio::canonical_uuid(u).expect("uuid")).collect();
    let mut b = board_with_scores(paths, &["#fake".to_owned()]);
    report("entity_removed_1000", label, measure(removed.len(), || removed.iter().for_each(|u| b.remove_entity(*u))));
}

/// 300 members in 3 teams: team and decorated name of the members and of 300 others; then 300
/// joins each followed by a push lookup (`execute as @e run team join t @s` among mobs).
fn team_cases(label: &str, paths: BoardPaths) {
    let members = uuids(300, 11);
    let others = uuids(300, 13);
    let mut b = Board::with_paths(paths);
    for (i, chunk) in members.chunks(100).enumerate() {
        b.add_team(team(&format!("t{i}"), chunk.to_vec()));
    }
    let lookups: Vec<&String> = members.iter().chain(&others).collect();
    report("team_of_300", label, measure(lookups.len(), || lookups.iter().for_each(|h| _ = black_box(b.team_of(h)))));
    report(
        "decorate_300",
        label,
        measure(lookups.len(), || lookups.iter().for_each(|h| drop(black_box(b.decorate(h, Component::text("n")))))),
    );
    let uuids_of: Vec<u128> = members.iter().map(|m| crate::nbtio::canonical_uuid(m).expect("uuid")).collect();
    report(
        "join_then_push_300",
        label,
        measure(members.len(), || {
            for (m, u) in members.iter().zip(&uuids_of) {
                b.join_team(m, "t0");
                black_box(b.collision_rule_by_uuid(*u));
            }
            b.out.clear();
        }),
    );
}

#[test]
#[ignore = "measurement bench"]
fn bench_scores() {
    let off = BoardPaths { holder_map: false, one_lookup: false, member_index: false, uuid_set: false };
    let variants = [
        ("off", off),
        ("holder_map", BoardPaths { holder_map: true, ..off }),
        ("one_lookup", BoardPaths { one_lookup: true, ..off }),
        ("member_index", BoardPaths { member_index: true, ..off }),
        ("uuid_set", BoardPaths { uuid_set: true, ..off }),
        ("holder_map_one_lookup", BoardPaths { holder_map: true, one_lookup: true, ..off }),
        ("all", BoardPaths { holder_map: true, one_lookup: true, member_index: true, uuid_set: true }),
    ];
    for (label, paths) in variants {
        // Boards made by `Board::default` (none here) would follow the same paths.
        FORCED_PATHS.with(|f| f.set(Some(paths)));
        for n in [1000, 10_000] {
            holder_cases(label, paths, n);
        }
        removal_case(label, paths);
        team_cases(label, paths);
    }
    FORCED_PATHS.with(|f| f.set(None));
}
