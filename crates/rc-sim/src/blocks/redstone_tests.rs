//! Weak dust signal read on one side only (`RUSTCRAFT_OPT_WIRESIDE`): differential
//! against the recomputed full state, and micro-benches.

use super::super::{Dir, P, above, flag};
use crate::server::Server;

/// Bench model world (`RUSTCRAFT_OPT_WORLD`, otherwise `target/perf/model`).
fn model_dir() -> Option<std::path::PathBuf> {
    if let Some(p) = rc_data::env_os!("RUSTCRAFT_OPT_WORLD") {
        return Some(p.into());
    }
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/perf/model");
    p.join("level.dat").is_file().then_some(p)
}

/// Server on a copy of the model world, chunks around `c` loaded; `None` without data
/// (explicit failure with `RUSTCRAFT_OPT_REQUIRE=1`).
fn open(tag: &str, c: P) -> Option<Server> {
    let model = rc_testdata::need(model_dir(), "bench model world")?;
    let data = std::sync::Arc::new(rc_testdata::need(rc_testdata::version_data(), "26.3 data cache")?);
    let dir = crate::tests::fresh_copy(&model, tag);
    let mut s = crate::portal_tests::open_world(&dir, data);
    let (cx, cz) = (c[0] >> 4, c[2] >> 4);
    let list: Vec<(i32, i32)> = (cx - 1..=cx + 1).flat_map(|x| (cz - 1..=cz + 1).map(move |z| (x, z))).collect();
    s.load_now(&list);
    Some(s)
}

fn close(mut s: Server) {
    s.stop_gen();
    s.stop_parked();
}

/// All the states of the named blocks.
fn states_of(s: &Server, names: &[&str]) -> Vec<u32> {
    let mut out = Vec::new();
    for n in names {
        let b = s.data.blocks.iter().find(|b| b.name == format!("minecraft:{n}")).unwrap_or_else(|| panic!("block {n}"));
        out.extend(b.first_state..b.first_state + b.state_count);
    }
    out
}

/// Reproducible generator (test seed, never the clock).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[(self.next() % v.len() as u64) as usize]
    }
}

/// Old answer: full recomputed state, then reading the side.
fn old_answer(s: &Server, st: u32, p: P, t: Dir) -> bool {
    s.wire_get(s.wire_connection_state(st, p), t).connected()
}

/// Differential: 3×3×3 neighbourhoods drawn at random (blocks that count for connections,
/// plus a quarter of arbitrary states), dust at the centre in an arbitrary stored state
/// (power and sides, one in four forced to a dot), 4 sides read and 6 directions of weak
/// signal: new == old. Fails if the "dot" condition is inverted.
#[test]
fn wireside_matches_full_state() {
    let c: P = [1600, 140, -448];
    let Some(mut s) = open("wireside", c) else { return };
    let wires = states_of(&s, &["redstone_wire"]);
    let palette = states_of(
        &s,
        &[
            "air",
            "stone",
            "glass",
            "glowstone",
            "redstone_wire",
            "redstone_wire",
            "redstone_wire",
            "repeater",
            "comparator",
            "observer",
            "redstone_torch",
            "redstone_wall_torch",
            "lever",
            "stone_button",
            "redstone_block",
            "target",
            "daylight_detector",
            "oak_trapdoor",
            "iron_trapdoor",
            "hopper",
            "oak_slab",
            "oak_stairs",
            "snow",
            "piston",
            "lightning_rod",
            "oak_leaves",
            "soul_sand",
            "dirt_path",
        ],
    );
    let all = s.data.states.len() as u64;
    let dot = |s: &Server, mut st: u32| {
        for d in Dir::HORIZONTAL {
            st = s.wire_set(st, d, super::Side::None);
        }
        st
    };
    let mut rng = Rng(crate::rng::test_seed() as u64 | 1);
    let (mut checked, mut dots, mut ext) = (0u64, 0u64, 0u64);
    let air = s.air();
    for k in 0..60_000 {
        // One draw in two sparse (mostly air): lines with a single connected side, where the
        // extension and the dot rule decide.
        let sparse = k % 2 == 0;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let q = [c[0] + dx, c[1] + dy, c[2] + dz];
                    let st = if sparse && !rng.next().is_multiple_of(8) {
                        air
                    } else if rng.next().is_multiple_of(4) {
                        (rng.next() % all) as u32
                    } else {
                        rng.pick(&palette)
                    };
                    s.write_raw(q, st);
                }
            }
        }
        let mut st = rng.pick(&wires);
        if rng.next().is_multiple_of(4) {
            st = dot(&s, st);
        }
        s.write_raw(c, st);
        s.relight.clear();
        let st = s.get(c);
        assert_eq!(s.v().block(st), s.bs.t.id.wire);
        let is_dot = s.wire_is_dot(st);
        for t in Dir::HORIZONTAL {
            let want = old_answer(&s, st, c, t);
            let got = s.wire_connected_side_only(st, c, t);
            assert_eq!(got, want, "side {t:?}, state {st}, dot {is_dot}");
            checked += 1;
            if is_dot {
                dots += 1;
            }
            // Side connected only by the line extension (no own neighbour on that side).
            let can_up = !s.v().conductor(s.get(above(c)));
            if want && !s.wire_side(c, t, can_up).connected() {
                ext += 1;
            }
        }
        for d in Dir::ALL {
            let new = s.wire_weak(st, c, d);
            let old = if !s.bs.wire_signal || d == Dir::Down {
                0
            } else {
                let power = s.v().get_int(st, "power");
                if power == 0 || (d != Dir::Up && !old_answer(&s, st, c, d.opposite())) { 0 } else { power }
            };
            assert_eq!(new, old, "weak signal {d:?}, state {st}");
        }
    }
    eprintln!("wireside: {checked} sides compared, of which {dots} on a dot, {ext} connected by extension");
    // Coverage: the two rare branches must have been exercised.
    assert!(dots > 10_000 && ext > 10_000, "insufficient coverage: dots {dots}, extensions {ext}");
    close(s);
}

/// Places a line of `n` dust on stone toward the east from `o` (source at `o`).
fn build_line(s: &mut Server, o: P, n: i32) {
    let stone = s.default_state("minecraft:stone");
    for i in -1..=n + 1 {
        for dz in -2..=2 {
            s.set_block([o[0] + i, o[1] - 1, o[2] + dz], stone, flag::ALL);
            for y in 0..3 {
                s.set_block([o[0] + i, o[1] + y, o[2] + dz], s.air(), flag::ALL);
            }
        }
    }
    for i in 1..=n {
        let p = [o[0] + i, o[1], o[2]];
        let st = s.wire_placement(p);
        s.set_block(p, st, flag::ALL);
    }
}

/// Micro-bench: answer for one side, old full computation against side alone, on the dust
/// of a line and of a comb (branches every 4 cells). 1 core, calling thread.
/// `cargo test --release -p rc-sim wireside_bench -- --ignored --nocapture`.
#[test]
#[ignore = "measurement bench"]
fn wireside_bench_calls() {
    let o: P = [1590, 140, -448];
    let Some(mut s) = open("wsbench", o) else { return };
    build_line(&mut s, o, 20);
    let wire = s.bs.t.id.wire;
    for i in (2..=20).step_by(4) {
        let p = [o[0] + i, o[1], o[2] + 1];
        let st = s.wire_placement(p);
        s.set_block(p, st, flag::ALL);
    }
    let mut cells: Vec<(u32, P)> = Vec::new();
    for i in 0..=21 {
        for dz in -1..=1 {
            let p = [o[0] + i, o[1], o[2] + dz];
            if s.v().block(s.get(p)) == wire {
                cells.push((s.get(p), p));
            }
        }
    }
    let reps = 1_000_000 / (cells.len() * 4) + 1;
    let n = (reps * cells.len() * 4) as f64;
    let run = |f: &dyn Fn(&Server, u32, P, Dir) -> bool| {
        let t = std::time::Instant::now();
        let mut acc = 0u32;
        for _ in 0..reps {
            for &(st, p) in &cells {
                for d in Dir::HORIZONTAL {
                    acc += f(std::hint::black_box(&s), st, p, d) as u32;
                }
            }
        }
        std::hint::black_box(acc);
        t.elapsed().as_secs_f64() * 1e9 / n
    };
    for pass in 0..3 {
        let old = run(&old_answer);
        let new = run(&|s, st, p, d| s.wire_connected_side_only(st, p, d));
        eprintln!("wireside pass {pass}: {} dust, old {old:.1} ns, side alone {new:.1} ns per call", cells.len());
    }
    close(s);
}

/// End-to-end micro-bench: line of 15 dust powered then cut 400 times by a
/// redstone block (propagation, neighbour updates, weak signals). Run twice,
/// with and without `RUSTCRAFT_OPT_WIRESIDE=0`. Also checks the 15..1 decay.
#[test]
#[ignore = "measurement bench"]
fn wireside_bench_toggle() {
    let o: P = [1590, 140, -448];
    let Some(mut s) = open("wstoggle", o) else { return };
    build_line(&mut s, o, 15);
    let rb = s.default_state("minecraft:redstone_block");
    let mut times = Vec::new();
    for k in 0..400 {
        let t = std::time::Instant::now();
        s.set_block(o, rb, flag::ALL);
        s.set_block(o, s.air(), flag::ALL);
        times.push(t.elapsed().as_secs_f64() * 1e6);
        s.relight.clear();
        s.changed.clear();
        if k == 0 {
            s.set_block(o, rb, flag::ALL);
            for i in 1..=15 {
                assert_eq!(s.v().get_int(s.get([o[0] + i, o[1], o[2]]), "power"), 16 - i, "dust {i}");
            }
            s.set_block(o, s.air(), flag::ALL);
        }
    }
    times.sort_by(f64::total_cmp);
    eprintln!(
        "wireside toggle (switch {}): median {:.1} µs, p99 {:.1} µs per power-on + cut",
        if super::wireside_on() { "1" } else { "0" },
        times[times.len() / 2],
        times[times.len() * 99 / 100]
    );
    close(s);
}
