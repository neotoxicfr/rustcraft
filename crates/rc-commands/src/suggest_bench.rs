//! Micro-bench of `RUSTCRAFT_OPT_SUGGESTFAST` on the client side, each lead alone then the whole
//! suggestion of a key press, former path then the new one, on the real tree and version data:
//! - building the list of the 9 000 objective criteria (`create` alone);
//! - listing the criteria (rebuilt at every key press, or kept);
//! - reading a slot argument (170 names rebuilt, or looked up);
//! - the whole local suggestion of a key press after `/give @s `, `/scoreboard objectives add x `,
//!   `/setblock ~ ~ ~ ` and `/` (parse, completions, merge).
//!
//! One thread: a key press is answered on the client's main thread, in a few milliseconds at
//! most once the list is built in one pass (no multi-core or GPU variant: nothing left to share).
//! Figures are printed, never checked. Run alone:
//! `cargo test --release -p rc-commands suggest_bench -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use crate::args::criteria_list;
use crate::parse::{Source, completions};
use crate::suggest::{Builder, Suggestions, force_fast_suggestions};
use crate::suggest_equivalence_tests::real_trees;
use crate::{Data, Tree, parse};

const SRC: Source = Source { restricted: true, selectors: true };

/// Median time of one call of `f` in µs, over `runs` timed runs of `calls` calls.
fn median_micros(runs: usize, calls: usize, mut f: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..calls {
                f();
            }
            t.elapsed().as_secs_f64() * 1e6 / calls as f64
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

/// The same measure on the former path then on the new one, as `[bench]` lines.
fn compare(case: &str, runs: usize, calls: usize, mut f: impl FnMut()) {
    let mut both = [0.0; 2];
    for (fast, slot) in [(false, 0), (true, 1)] {
        force_fast_suggestions(Some(fast));
        f();
        both[slot] = median_micros(runs, calls, &mut f);
    }
    force_fast_suggestions(None);
    eprintln!("[bench] suggest {case} former_us {:.1}", both[0]);
    eprintln!("[bench] suggest {case} new_us {:.1}", both[1]);
    eprintln!("[bench] suggest {case} gain_x {:.1}", both[0] / both[1]);
}

fn key_press(tree: &Tree, data: &Data, text: &str) -> Suggestions {
    let input: Vec<char> = text.chars().collect();
    let p = parse(tree, &input, data, SRC);
    let c = completions(tree, &p, &input, input.len(), data, SRC);
    Suggestions::merge(&input, c.local)
}

#[test]
#[ignore = "micro-bench: run alone with --ignored --nocapture"]
fn bench_suggestions() {
    let (trees, mut data) = real_trees();
    data.op_level = 4;
    let tree = &trees[4];
    let input: Vec<char> = "/scoreboard objectives add x ".chars().collect();
    let criteria = criteria_list(&data);
    eprintln!("[bench] suggest criteria count {}", criteria.len());
    let mut b = Builder::new(&input, input.len());
    b.suggest_matching(criteria.iter());
    let list = b.out;
    compare("create_9000_criteria", 5, 3, || {
        std::hint::black_box(Suggestions::create(&input, list.clone()));
    });
    compare("clone_9000_criteria_only", 5, 3, || {
        std::hint::black_box(list.clone());
    });
    let mut deduped = list.clone();
    crate::suggest::keep_first_occurrences(&mut deduped);
    compare("dedupe_9000_criteria_only", 5, 3, || {
        let mut l = list.clone();
        crate::suggest::keep_first_occurrences(&mut l);
        std::hint::black_box(l);
    });
    let mut in_order = deduped.clone();
    in_order.sort_by(|a, b| crate::suggest::cmp_ignore_case(&a.text, &b.text));
    for (case, unsorted) in [("registry_order", &deduped), ("already_in_order", &in_order)] {
        compare(&format!("sort_cached_key_9000_{case}"), 5, 3, || {
            let mut l = unsorted.clone();
            l.sort_by_cached_key(|s| crate::suggest::fold_case_key(&s.text));
            std::hint::black_box(l);
        });
        compare(&format!("sort_comparison_9000_{case}"), 5, 3, || {
            let mut l = unsorted.clone();
            l.sort_by(|a, b| crate::suggest::cmp_ignore_case(&a.text, &b.text));
            std::hint::black_box(l);
        });
    }
    // Block ids: in the order of the map (former path without its sort) or sorted first.
    let block_input: Vec<char> = "/setblock ~ ~ ~ ".chars().collect();
    let mut ids: Vec<&String> = data.blocks.keys().collect();
    let block_list = |ids: &[&String]| {
        let mut b = Builder::new(&block_input, block_input.len());
        b.suggest_resources(ids);
        b.out
    };
    let hash_order = block_list(&ids);
    compare("create_block_ids_map_order", 9, 20, || {
        std::hint::black_box(Suggestions::create(&block_input, hash_order.clone()));
    });
    compare("sort_block_ids_alone", 9, 20, || {
        let mut v = ids.clone();
        v.sort();
        std::hint::black_box(v);
    });
    ids.sort();
    let sorted_order = block_list(&ids);
    compare("create_block_ids_sorted_first", 9, 20, || {
        std::hint::black_box(Suggestions::create(&block_input, sorted_order.clone()));
    });
    compare("list_criteria", 9, 20, || {
        let d = std::hint::black_box(&data);
        if crate::suggest::fast_suggestions() {
            std::hint::black_box(d.objective_criteria(criteria_list).len());
        } else {
            std::hint::black_box(criteria_list(d).len());
        }
    });
    let slot: Vec<char> = "/item replace entity @s armor.head with minecraft:stone".chars().collect();
    compare("parse_slot_command", 9, 2_000, || {
        std::hint::black_box(parse(tree, &slot, &data, SRC));
    });
    for (case, text) in [
        ("key_press_root", "/"),
        ("key_press_give_target", "/give @s "),
        ("key_press_give_item", "/give @s minecraft:"),
        ("key_press_criteria", "/scoreboard objectives add x "),
        ("key_press_setblock", "/setblock ~ ~ ~ "),
        ("key_press_fill_tag_props", "/fill ~ ~ ~ ~ ~ ~ stone replace #logs["),
    ] {
        compare(case, 5, 5, || {
            std::hint::black_box(key_press(tree, &data, text));
        });
    }
}
