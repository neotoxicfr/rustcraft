//! Micro-bench of `RUSTCRAFT_OPT_SUGGESTFAST` on the integrated server: one suggestion request
//! (`compute_suggestions`, what the server answers to a key press on an argument it is asked for)
//! after `/give @s `, `/scoreboard objectives add x `, `/setblock ~ ~ ~ `, `/function ` and `/`,
//! former path (tree copied, quadratic lists) then the new one, on the flat world of the
//! equivalence proof.
//!
//! One thread: a request is answered on the server thread, at most one per tick (no multi-core
//! or GPU variant). Figures are printed, never checked. Run alone:
//! `cargo test --release -p rc-sim bench_command_suggestions -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use rc_commands::suggest::force_fast_suggestions;

use crate::bench_kit::{median, report};

#[test]
#[ignore = "micro-bench: run alone with --ignored --nocapture"]
fn bench_command_suggestions() {
    let run = || {
        let (mut p, dir) = crate::commands::suggestion_equivalence_tests::prepared_world("suggest-bench");
        for (case, text) in [
            ("root", "/"),
            ("give_target", "/give @s "),
            ("criteria", "/scoreboard objectives add x "),
            ("setblock", "/setblock ~ ~ ~ "),
            ("function", "/function "),
        ] {
            for (path, fast) in [("former_us", false), ("new_us", true)] {
                force_fast_suggestions(Some(fast));
                std::hint::black_box(p.s.compute_suggestions(text, 4));
                let times: Vec<f64> = (0..7)
                    .map(|_| {
                        let t = Instant::now();
                        std::hint::black_box(p.s.compute_suggestions(text, 4));
                        t.elapsed().as_secs_f64() * 1e6
                    })
                    .collect();
                report("command_suggestions", case, path, median(&times));
            }
            force_fast_suggestions(None);
        }
        crate::tests_g3f13::finish(p, dir);
    };
    std::thread::Builder::new()
        .stack_size(crate::server::SOLO_STACK)
        .spawn(run)
        .expect("thread")
        .join()
        .expect("bench run");
}
