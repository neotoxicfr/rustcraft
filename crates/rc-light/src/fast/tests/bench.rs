//! Relight micro-bench (ignored by default): `cargo test --release -p rc-light bench_relight
//! -- --ignored --nocapture`. World of 7 × 7 chunks 384 high (stone plain up to
//! y = 128, caves, foliage), full initial light; traces: block placed then removed at the
//! surface (plains), in caves (cave), lamp turned on then off (redstone, sky
//! filtered by `matters_sky`), 30-block building site (building). Figure: µs per relight,
//! best of 3 passes, old path, each switch alone, all, all except one.

use super::*;

const H: i32 = 384;
const R: i32 = 3;

fn bench_world(t: &LightTables) -> World {
    let mut rng = Rng(0x5eed);
    let mut cols: hashbrown::HashMap<(i32, i32), Vec<u32>> = hashbrown::HashMap::new();
    let caves: Vec<[i32; 4]> =
        (0..60).map(|_| [rng.below(112) - 48, 40 + rng.below(70), rng.below(112) - 48, 3 + rng.below(5)]).collect();
    let trees: Vec<[i32; 2]> = (0..40).map(|_| [rng.below(112) - 48, rng.below(112) - 48]).collect();
    for cx in -R..=R {
        for cz in -R..=R {
            let mut v = vec![0u32; (256 * H) as usize];
            for y in 0..H {
                for z in 0..16 {
                    for x in 0..16 {
                        let (wx, wz) = (cx * 16 + x, cz * 16 + z);
                        let mut s = if y < 128 { 1 } else { 0 };
                        if caves.iter().any(|c| (wx - c[0]).pow(2) + (y - c[1]).pow(2) + (wz - c[2]).pow(2) < c[3] * c[3]) {
                            s = 0;
                        }
                        if trees.iter().any(|t| (wx - t[0]).abs() <= 2 && (wz - t[1]).abs() <= 2 && (132..136).contains(&y)) {
                            s = 2;
                        }
                        v[((y << 8) | (z << 4) | x) as usize] = s;
                    }
                }
            }
            cols.insert((cx, cz), v);
        }
    }
    let mut m = hashbrown::HashMap::new();
    for cx in -R..=R {
        for cz in -R..=R {
            let refs: crate::Columns = std::array::from_fn(|i| {
                let (dx, dz) = (i as i32 % 3 - 1, i as i32 / 3 - 1);
                cols.get(&(cx + dx, cz + dz)).map(|v| &v[..])
            });
            let (sky, block) = crate::light_chunk(t, refs, H, true);
            let v = &cols[&(cx, cz)];
            let secs = (0..(H / 16) as usize)
                .map(|s| {
                    let vals = &v[s * 4096..(s + 1) * 4096];
                    let blocks = rc_world::Container::pack(vals, 4, 8, 15);
                    Section { non_empty: vals.iter().filter(|&&x| x != 0).count() as i16, blocks, biomes: rc_world::Container::Single(0) }
                })
                .collect();
            m.insert((cx, cz), (secs, sky, block));
        }
    }
    World(m)
}

/// A trace: lists of (changes, placed state); each step is followed by its inverse.
fn trace(world: &World, kind: &str) -> Vec<Vec<([i32; 3], u32)>> {
    let mut rng = Rng(42);
    let get = |p: [i32; 3]| {
        let (s, _, _) = &world.0[&(p[0] >> 4, p[2] >> 4)];
        s[(p[1] >> 4) as usize].blocks.get(rc_world::block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize))
    };
    let mut out = Vec::new();
    while out.len() < 60 {
        let (x, z) = (rng.below(32) - 16, rng.below(32) - 16);
        match kind {
            "plains" | "torches" => out.push(vec![([x, 128, z], 1)]),
            "redstone" => {
                // Lamp off placed once (outside the measurement) then turned on and off.
                out.push(vec![([x, 128, z], LAMP_ON)]);
            }
            "water" => {
                // Flow front: 12 neighbouring blocks (one group), like a sheet that spreads.
                out.push((0..12).map(|k| ([x + k % 4, 128, z + k / 4], 2)).collect());
            }
            "slab" => {
                // 16 × 16 slab in the air: large shadow, large zone (a group of 256).
                out.push((0..256).map(|k| ([x + k % 16, 150, z + k / 16], 1)).collect());
            }
            "spread" => {
                // Four placements scattered over the whole world (often independent groups).
                let p = |rng: &mut Rng| [rng.below(100) - 48, 128, rng.below(100) - 48];
                out.push((0..4).map(|_| (p(&mut rng), 1)).collect());
            }
            "cave" => {
                let p = [x, 40 + rng.below(70), z];
                if get(p) == 0 && (0..6).any(|d| get([p[0] + STEP[d][0], p[1] + STEP[d][1], p[2] + STEP[d][2]]) == 1) {
                    out.push(vec![(p, 1)]);
                }
            }
            _ => {
                let mut g = Vec::new();
                for k in 0..30 {
                    g.push(([x + k % 6, 128 + k / 18, z + (k / 6) % 3], 1));
                }
                out.push(g);
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Path {
    Old,
    New(Opts),
}

/// µs per relight (best of `passes`) for a path on a trace.
fn run(t: &LightTables, base: &World, tr: &[Vec<([i32; 3], u32)>], path: Path, masked: bool, passes: usize) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..passes {
        let mut w = base.clone();
        let mut ns = 0u128;
        let mut count = 0;
        for step in tr {
            for undo in [false, true] {
                let mut ps = Vec::new();
                let mut sky = Vec::new();
                for &(p, s) in step {
                    let s = if undo { if s == LAMP_ON { LAMP_OFF } else { 0 } } else { s };
                    let old = set(&mut w, p, s).unwrap_or(0);
                    ps.push(p);
                    if t.matters_sky(old, s) {
                        sky.push(p);
                    }
                }
                let t0 = std::time::Instant::now();
                match path {
                    Path::Old => {
                        std::hint::black_box(crate::relight_zones(&mut w, H, t, true, &ps, true));
                    }
                    Path::New(o) => {
                        let mut out = [Touched::new()];
                        let sky = masked.then_some(&sky[..]);
                        relight_zones(&mut w, H, t, true, &ps, sky, o, &mut out);
                        std::hint::black_box(&out);
                    }
                }
                ns += t0.elapsed().as_nanos();
                count += 1;
            }
        }
        best = best.min(ns as f64 / count as f64 / 1000.0);
    }
    best
}

/// Targeted A/B: all against all except one switch, 15 interleaved passes.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_ab -- --ignored --nocapture"]
fn bench_ab() {
    let t = tables();
    let base = bench_world(&t);
    let all = !DEDUP;
    let names = ["SKYMEMO", "BLOCKEARLY", "ZONEBUF", "LIGHTQDIR", "RELIGHT_OCCTAB"];
    for kind in ["plains", "cave", "building"] {
        let tr = trace(&base, kind);
        let mut paths = vec![("all".to_string(), Opts::from_bits(all))];
        for n in names {
            let k = NAMES.iter().position(|x| *x == n).expect("name");
            paths.push((format!("all except {n}"), Opts::from_bits(all & !(1 << k))));
        }
        paths.push(("all at 0".into(), Opts::from_bits(0)));
        paths.push(("LIGHTQDIR alone".into(), Opts::from_bits(256)));
        let mut best = vec![f64::MAX; paths.len()];
        for _ in 0..15 {
            for (b, (_, o)) in best.iter_mut().zip(&paths) {
                *b = b.min(run(&t, &base, &tr, Path::New(*o), false, 1));
            }
        }
        for ((name, _), b) in paths.iter().zip(&best) {
            eprintln!("{kind:9} {name:28} {b:9.1} µs ({:+.1} % against all)", (b / best[0] - 1.0) * 100.0);
        }
    }
}

/// Starting world of a trace (lamps off in place for `redstone`).
fn start_world(t: &LightTables, base: &World, kind: &str) -> World {
    let mut world = base.clone();
    if kind == "redstone" {
        for step in trace(base, kind) {
            for (p, _) in step {
                set(&mut world, p, LAMP_OFF);
            }
        }
        let ps: Vec<[i32; 3]> = trace(base, kind).into_iter().flatten().map(|(p, _)| p).collect();
        crate::relight_zones(&mut world, H, t, true, &ps, true);
    }
    if kind == "torches" {
        // Torches placed on the ground around the changes: the block has light to spread.
        let mut rng = Rng(7);
        let ps: Vec<[i32; 3]> = (0..40).map(|_| [rng.below(40) - 20, 128, rng.below(40) - 20]).collect();
        for &p in &ps {
            set(&mut world, p, TORCH);
        }
        crate::relight_zones(&mut world, H, t, true, &ps, true);
    }
    world
}

/// Share of each phase of a sky resolution (zone read, sources + seeds,
/// propagation, write), sequential path without then with the C2-M02 switches (twice, A B A B),
/// µs per group: where multi-core can win.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_phases -- --ignored --nocapture"]
fn bench_phases() {
    let t = tables();
    let base = bench_world(&t);
    // Sequential switches of C2-M03, then with those of C2-M02 (frontier seeding, step order),
    // then without one of the seeding switches; the list, then the list backwards.
    let shipped = 0x7ff | 0xf << 14 | ZONE_IO_BITS | SEED_AND_SKY_COLUMN_BITS;
    let paths = [
        ("before C2-M02", 0x7ff),
        ("shipped without C2-M10", shipped & !ZONE_IO_BITS),
        ("shipped without C2-M11 (series 3)", shipped & !SEED_AND_SKY_COLUMN_BITS),
        ("shipped", shipped),
        ("shipped without LIGHTSEEDROWS", shipped & !(1 << 16)),
        ("shipped without LIGHTMARGINFRONTIER", shipped & !(1 << 15)),
        ("shipped without LIGHTLEVELFIRST", shipped & !(1 << 17)),
    ];
    for (label, bits) in paths.iter().chain(paths.iter().rev()) {
        eprintln!("{label}:");
        phases(&t, &base, Opts::from_bits(!super::DEFAULT_OFF & bits));
    }
}

/// Phases of the sky resolutions of each trace with the switches `o`.
fn phases(t: &LightTables, base: &World, o: Opts) {
    let c = Ctx { height: H, t, o, count_queues: false };
    for kind in ["plains", "torches", "building", "slab", "spread"] {
        let mut world = start_world(t, base, kind);
        let mut ns = [0u128; 5];
        let mut cells = 0usize;
        let mut count = 0usize;
        for step in trace(base, kind) {
            for undo in [false, true] {
                let ps: Vec<[i32; 3]> = step.iter().map(|&(p, _)| p).collect();
                for &(p, s) in &step {
                    set(&mut world, p, if undo { 0 } else { s });
                }
                for g in groups(&ps) {
                    let mut out = [Touched::new()];
                    with_work(true, |w| {
                        let t0 = std::time::Instant::now();
                        let bg = block_zone(&c, &g);
                        let (sg, _) = sky_zone(&world, &c, &mut w.aux, &g, bg);
                        let t1 = std::time::Instant::now();
                        read(&world, &c, w, sg, true, true);
                        let t2 = std::time::Instant::now();
                        let Work { face, prop, emit, emitters, sky, aux, lane, .. } = w;
                        let z = Cells { face, prop, emit, emitters };
                        seed(&world, &c, &z, sky, lane, Some(aux), None, sg, 0, sg.size[1], true);
                        let t3 = std::time::Instant::now();
                        propagate(&c, &z, sky, lane, sg, 0, sg.size[1]);
                        let t4 = std::time::Instant::now();
                        write(&mut world, &c, sky, lane, sg, 0, sg.size[1], true, &mut out);
                        let t5 = std::time::Instant::now();
                        for (k, d) in [t1 - t0, t2 - t1, t3 - t2, t4 - t3, t5 - t4].iter().enumerate() {
                            ns[k] += d.as_nanos();
                        }
                        cells += sg.cells();
                    });
                    count += 1;
                }
            }
        }
        let us = |v: u128| v as f64 / 1000.0 / count as f64;
        eprintln!(
            "{kind:9} {count} groups, {} zone cells: zones {:.1}, read {:.1}, sources+seeds {:.1}, propagation {:.1}, write {:.1} µs",
            cells / count,
            us(ns[0]),
            us(ns[1]),
            us(ns[2]),
            us(ns[3]),
            us(ns[4])
        );
    }
}

/// Multi-core (C2-M11): µs per relight, best of 5 interleaved passes. First called outside the
/// pool (game case: server thread, light pool at `RUSTCRAFT_THREADS` threads), each
/// approach alone and all, shipped thresholds and thresholds at 0; then in pools of 1 to 32 threads.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_par -- --ignored --nocapture"]
fn bench_par() {
    let t = tables();
    let base = bench_world(&t);
    let seq = !super::DEFAULT_OFF & 0x7ff;
    let o = |bits: u32, zero: bool| {
        let o = Opts::from_bits(seq | bits);
        if zero { Opts { join_min: 0, par_min: 0, ..o } } else { o }
    };
    let paths = [
        ("sequential (shipped C2-M03)", o(0, false)),
        ("LIGHTJOIN", o(2048, false)),
        ("LIGHTJOIN threshold 0", o(2048, true)),
        ("LIGHTPARSOLVE", o(4096, false)),
        ("LIGHTPARSOLVE threshold 0", o(4096, true)),
        ("LIGHTPARSOLVE threshold 32k", Opts { par_min: 32_768, ..o(4096, false) }),
        ("LIGHTPARSOLVE threshold 64k", Opts { par_min: 65_536, ..o(4096, false) }),
        ("LIGHTPARSOLVE 64k tasks 512", Opts { par_min: 65_536, chunk: 512, ..o(4096, false) }),
        ("LIGHTPARSOLVE 64k tasks 64", Opts { par_min: 65_536, chunk: 64, ..o(4096, false) }),
        ("LIGHTGROUPS", o(8192, false)),
        ("LIGHTGROUPS threshold 0", o(8192, true)),
        ("JOIN+GROUPS", o(2048 | 8192, false)),
        ("all", o(PAR_BITS, false)),
        ("all thresholds 0", o(PAR_BITS, true)),
    ];
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let only = rc_data::env_str!("RUSTCRAFT_BENCH_KINDS");
    for kind in ["plains", "torches", "cave", "redstone", "building", "water", "slab", "spread"] {
        if only.is_some_and(|k| !k.split(',').any(|x| x == kind)) {
            continue;
        }
        let world = start_world(&t, &base, kind);
        let tr = trace(&base, kind);
        let masked = kind == "redstone";
        let mut best = vec![f64::MAX; paths.len()];
        for _ in 0..5 {
            for (b, (_, p)) in best.iter_mut().zip(&paths) {
                *b = b.min(run(&t, &world, &tr, Path::New(*p), masked, 1));
            }
        }
        for ((name, p), b) in paths.iter().zip(&best) {
            let before = crate::stats::totals();
            let total = run(&t, &world, &tr, Path::New(*p), masked, 1);
            let st = crate::stats::totals().since(&before);
            let per = |ns: u64| ns as f64 / 1000.0 / (2 * tr.len()) as f64;
            let kept = super::WORK.with(|w| w.try_borrow().map_or(0, |w| w.bytes()));
            eprintln!(
                "{kind:9} outside pool {name:28} {b:9.1} µs ({:+.1} %); next pass {total:.1} µs of which sky {:.1} ({} res.), block {:.1} ({} res.), {kept} B kept",
                (b / best[0] - 1.0) * 100.0,
                per(st.sky_ns),
                st.sky_solves,
                per(st.block_ns),
                st.block_solves
            );
        }
        let mut n = if rc_data::env_flag!("RUSTCRAFT_BENCH_NOPOOLS") { usize::MAX } else { 1 };
        while n <= cores.max(32) {
            let pool = super::pool(n);
            let mut bp = vec![f64::MAX; paths.len()];
            for _ in 0..3 {
                for (b, (_, p)) in bp.iter_mut().zip(&paths) {
                    *b = b.min(pool.install(|| run(&t, &world, &tr, Path::New(*p), masked, 1)));
                }
            }
            let line: Vec<String> =
                paths.iter().zip(&bp).skip(1).map(|((name, _), b)| format!("{name} {:+.0} %", (b / bp[0] - 1.0) * 100.0)).collect();
            eprintln!("{kind:9} pool {n:2} threads: sequential {:9.1} µs; {}", bp[0], line.join(", "));
            n *= 2;
        }
    }
}

/// Switches of C2-M02 (frontier seeding, step order, parallel step): bits 14 to 21.
const PROPAGATION_SWITCHES: u32 = 0xff << 14;

/// Shipped switches, `LIGHTDEDUP` aside.
fn shipped_bits() -> u32 {
    !super::DEFAULT_OFF & ((1u32 << NAMES.len()) - 1) & !DEDUP
}

/// µs per relight of each path, best of `passes`, paths run forward then backward on
/// alternate passes (A B B A: a slow phase of the machine weighs on every path alike).
fn best_abba(t: &LightTables, world: &World, tr: &[Vec<([i32; 3], u32)>], paths: &[Opts], masked: bool, passes: usize) -> Vec<f64> {
    let mut best = vec![f64::MAX; paths.len()];
    for pass in 0..passes {
        let order: Vec<usize> = if pass % 2 == 0 { (0..paths.len()).collect() } else { (0..paths.len()).rev().collect() };
        for k in order {
            best[k] = best[k].min(run(t, world, tr, Path::New(paths[k]), masked, 1));
        }
    }
    best
}

/// C2-M02: shipped configuration with every C2-M02 switch off ("before"), shipped, each switch
/// alone on "before" and all but it, then the thresholds (`LIGHTSPREADMIN`, `LIGHTPARMIN`,
/// `LIGHTCHUNK`); outside a pool (calling thread and light pool: the game's case), then the
/// light pool's size (pools of 1, 2, 4, 8 threads) for before and shipped. µs per relight, best of 7.
/// `RUSTCRAFT_BENCH_KINDS=slab,redstone` keeps some traces.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_propagation_switches -- --ignored --nocapture"]
fn bench_propagation_switches() {
    let t = tables();
    let base = bench_world(&t);
    let shipped = shipped_bits();
    let before = shipped & !PROPAGATION_SWITCHES;
    let on = Opts::from_bits(shipped);
    let mut paths = vec![("before (C2-M02 off)".to_string(), Opts::from_bits(before)), ("shipped".to_string(), on)];
    for (k, name) in NAMES.iter().enumerate().skip(14) {
        paths.push((format!("{name} alone"), Opts::from_bits(before | 1 << k)));
        paths.push((format!("all except {name}"), Opts::from_bits(shipped & !(1 << k))));
    }
    // The parallel step alone (inline, atomic view, kept queues): long fronts without the frontier.
    paths.push(("parallel step (3 switches) alone".into(), Opts::from_bits(before | 0x7 << 18)));
    for spread_min in [2048, 32_768, usize::MAX] {
        paths.push((format!("LIGHTSPREADMIN={spread_min}"), Opts { spread_min, ..on }));
    }
    for par_min in [131_072, usize::MAX] {
        paths.push((format!("LIGHTPARMIN={par_min}"), Opts { par_min, ..on }));
    }
    for chunk in [1024, 4096] {
        paths.push((format!("LIGHTCHUNK={chunk}"), Opts { chunk, ..on }));
    }
    let opts: Vec<Opts> = paths.iter().map(|(_, o)| *o).collect();
    let only = rc_data::env_str!("RUSTCRAFT_BENCH_KINDS");
    for kind in ["plains", "torches", "cave", "redstone", "building", "water", "slab", "spread"] {
        if only.is_some_and(|k| !k.split(',').any(|x| x == kind)) {
            continue;
        }
        let world = start_world(&t, &base, kind);
        let tr = trace(&base, kind);
        let masked = kind == "redstone";
        let best = best_abba(&t, &world, &tr, &opts, masked, 7);
        for ((name, _), b) in paths.iter().zip(&best) {
            eprintln!("{kind:9} outside pool {name:34} {b:9.1} µs ({:+.1} % against before)", (b / best[0] - 1.0) * 100.0);
        }
        for n in [1, 2, 4, 8] {
            let pool = super::pool(n);
            let b = pool.install(|| best_abba(&t, &world, &tr, &opts[..2], masked, 5));
            eprintln!("{kind:9} pool {n} threads: before {:9.1} µs, shipped {:9.1} µs ({:+.1} %)", b[0], b[1], (b[1] / b[0] - 1.0) * 100.0);
        }
    }
}

/// Queue entries per relight with the C2-M02 seeding off and on (deterministic: any machine),
/// sky and block, on the micro-bench traces; same sheets checked at every relight.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_queue_entries -- --ignored --nocapture"]
fn bench_queue_entries() {
    let t = tables();
    let base = bench_world(&t);
    let shipped = shipped_bits();
    let before = Opts::from_bits(shipped & !PROPAGATION_SWITCHES);
    let paths = [("before", before), ("shipped", Opts::from_bits(shipped))];
    for kind in ["plains", "torches", "cave", "redstone", "building", "water", "slab", "spread"] {
        let world = start_world(&t, &base, kind);
        let tr = trace(&base, kind);
        let masked = kind == "redstone";
        let mut sheets = Vec::new();
        for (name, o) in paths {
            // Decided by the calling thread for the whole relight, pool threads included.
            crate::stats::force_queue_counting(Some(true));
            let q0 = crate::stats::queues();
            let mut w = world.clone();
            replay_micro(&t, &mut w, &tr, o, masked);
            let q = crate::stats::queues().since(&q0);
            crate::stats::force_queue_counting(None);
            sheets.push(w);
            let n = (2 * tr.len()) as f64;
            for (label, k) in [("sky", crate::stats::SKY), ("block", crate::stats::BLOCK)] {
                let seeded: u64 = q.seeded[k].iter().sum();
                let queued: u64 = q.queued[k].iter().sum();
                eprintln!(
                    "{kind:9} {name:8} {label:5} per relight: seeded {:9.0} (at 15: {:9.0}), entries {:9.0}, cells {:9.0}",
                    seeded as f64 / n,
                    q.seeded[k][15] as f64 / n,
                    queued as f64 / n,
                    q.cells[k] as f64 / n
                );
            }
        }
        assert!(sheets[0] == sheets[1], "{kind}: sheets differ");
    }
}

/// µs per sky group of the zone read and of the write (changed cells found, then written),
/// replaying `kind` from `world` with the switches `o` (the trace of [`phases`]).
fn read_and_write_us(t: &LightTables, base: &World, world: &World, kind: &str, o: Opts) -> [f64; 2] {
    let c = Ctx { height: H, t, o, count_queues: false };
    let mut world = world.clone();
    let (mut read_ns, mut write_ns, mut count) = (0u128, 0u128, 0usize);
    for step in trace(base, kind) {
        for undo in [false, true] {
            let ps: Vec<[i32; 3]> = step.iter().map(|&(p, _)| p).collect();
            for &(p, s) in &step {
                set(&mut world, p, if undo { 0 } else { s });
            }
            for g in groups(&ps) {
                let mut out = [Touched::new()];
                with_work(true, |w| {
                    let bg = block_zone(&c, &g);
                    let (sg, _) = sky_zone(&world, &c, &mut w.aux, &g, bg);
                    let t0 = std::time::Instant::now();
                    read(&world, &c, w, sg, true, true);
                    read_ns += t0.elapsed().as_nanos();
                    let Work { face, prop, emit, emitters, sky, aux, lane, .. } = w;
                    let z = Cells { face, prop, emit, emitters };
                    seed(&world, &c, &z, sky, lane, Some(aux), None, sg, 0, sg.size[1], true);
                    propagate(&c, &z, sky, lane, sg, 0, sg.size[1]);
                    let t1 = std::time::Instant::now();
                    write(&mut world, &c, sky, lane, sg, 0, sg.size[1], true, &mut out);
                    write_ns += t1.elapsed().as_nanos();
                });
                count += 1;
            }
        }
    }
    [read_ns, write_ns].map(|ns| ns as f64 / 1000.0 / count as f64)
}

/// [`read_and_write_us`] of each path, best of `passes`, paths forward then backward on alternate
/// passes (A B B A).
fn best_read_write_abba(t: &LightTables, base: &World, world: &World, kind: &str, paths: &[Opts], passes: usize) -> Vec<[f64; 2]> {
    let mut best = vec![[f64::MAX; 2]; paths.len()];
    for pass in 0..passes {
        let order: Vec<usize> = if pass % 2 == 0 { (0..paths.len()).collect() } else { (0..paths.len()).rev().collect() };
        for k in order {
            let [read_us, write_us] = read_and_write_us(t, base, world, kind, paths[k]);
            best[k] = [best[k][0].min(read_us), best[k][1].min(write_us)];
        }
    }
    best
}

/// C2-M10: zone read and write of the sky resolutions (µs per group) before C2-M10 (its four
/// switches off), shipped, each switch alone on before and shipped without each, best of 7 A B B A;
/// then whole relights before / shipped (µs per relight, best of 7) on every trace; then the slab
/// (zones of 142,500 cells, read in parallel bands) in pools of 1, 2, 4… threads up to every core.
/// `RUSTCRAFT_BENCH_KINDS=slab,cave` keeps some traces.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_zone_io -- --ignored --nocapture"]
fn bench_zone_io() {
    let t = tables();
    let base = bench_world(&t);
    let shipped = Opts::from_bits(shipped_bits());
    let before = Opts::from_bits(shipped_bits() & !ZONE_IO_BITS);
    let mut paths = vec![("before C2-M10".to_string(), before), ("shipped".to_string(), shipped)];
    for name in ["LIGHTREADROWS", "LIGHTREADFILL", "LIGHTSCANPLANES", "LIGHTWRITECOLUMN", "LIGHTZEROONLYRING"] {
        paths.push((format!("{name} alone"), before.with(name, true)));
        // A switch off by default is measured added to the shipped set, the others taken out of it.
        let shipped_on = shipped.with(name, true) == shipped;
        let label = if shipped_on { "shipped without" } else { "shipped with" };
        paths.push((format!("{label} {name}"), shipped.with(name, !shipped_on)));
    }
    let opts: Vec<Opts> = paths.iter().map(|(_, o)| *o).collect();
    let only = rc_data::env_str!("RUSTCRAFT_BENCH_KINDS");
    let wanted = |kind: &str| only.is_none_or(|k| k.split(',').any(|x| x == kind));
    for kind in ["plains", "torches", "cave", "building", "slab", "spread"].into_iter().filter(|k| wanted(k)) {
        let world = start_world(&t, &base, kind);
        let best = best_read_write_abba(&t, &base, &world, kind, &opts, 7);
        let [read0, write0] = best[0];
        for ((name, _), [read_us, write_us]) in paths.iter().zip(&best) {
            eprintln!(
                "{kind:9} {name:34} read {read_us:7.1} µs ({:+.1} %), write {write_us:6.1} µs ({:+.1} %)",
                (read_us / read0 - 1.0) * 100.0,
                (write_us / write0 - 1.0) * 100.0
            );
        }
    }
    for kind in ["plains", "torches", "cave", "redstone", "building", "water", "slab", "spread"].into_iter().filter(|k| wanted(k)) {
        let world = start_world(&t, &base, kind);
        let tr = trace(&base, kind);
        let b = best_abba(&t, &world, &tr, &[before, shipped], kind == "redstone", 7);
        eprintln!("{kind:9} relight: before {:9.1} µs, shipped {:9.1} µs ({:+.1} %)", b[0], b[1], (b[1] / b[0] - 1.0) * 100.0);
    }
    if wanted("slab") {
        let world = start_world(&t, &base, "slab");
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut threads: Vec<usize> = std::iter::successors(Some(1), |n| Some(n * 2)).take_while(|&n| n <= cores).collect();
        if threads.last() != Some(&cores) {
            threads.push(cores);
        }
        for n in threads {
            let b = super::pool(n).install(|| best_read_write_abba(&t, &base, &world, "slab", &opts[..2], 5));
            eprintln!(
                "slab      pool {n:2} threads: read before {:7.1} shipped {:7.1} µs ({:+.1} %), write before {:6.1} shipped {:6.1} µs ({:+.1} %)",
                b[0][0],
                b[1][0],
                (b[1][0] / b[0][0] - 1.0) * 100.0,
                b[0][1],
                b[1][1],
                (b[1][1] / b[0][1] - 1.0) * 100.0
            );
        }
    }
}

/// Replays a micro-bench trace (each step then its inverse) with the switches `o`.
fn replay_micro(t: &LightTables, w: &mut World, tr: &[Vec<([i32; 3], u32)>], o: Opts, masked: bool) {
    for step in tr {
        for undo in [false, true] {
            let mut ps = Vec::new();
            let mut sky = Vec::new();
            for &(p, s) in step {
                let s = if undo { if s == LAMP_ON { LAMP_OFF } else { 0 } } else { s };
                let old = set(w, p, s).unwrap_or(0);
                ps.push(p);
                if t.matters_sky(old, s) {
                    sky.push(p);
                }
            }
            let mut out = [Touched::new()];
            relight_zones(w, H, t, true, &ps, masked.then_some(&sky[..]), o, &mut out);
        }
    }
}

#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_relight -- --ignored --nocapture"]
fn bench_relight() {
    let t = tables();
    let base = bench_world(&t);
    let all = !DEDUP;
    for kind in ["plains", "cave", "redstone", "building"] {
        let mut world = base.clone();
        if kind == "redstone" {
            // Lamps off in place (the bench's starting world).
            for step in trace(&base, kind) {
                for (p, _) in step {
                    set(&mut world, p, LAMP_OFF);
                }
            }
            let ps: Vec<[i32; 3]> = trace(&base, kind).into_iter().flatten().map(|(p, _)| p).collect();
            crate::relight_zones(&mut world, H, &t, true, &ps, true);
        }
        let tr = trace(&base, kind);
        let masked = kind == "redstone";
        // Paths measured in interleaved passes (machine load varies): best of 7.
        let mut paths: Vec<(String, Path)> =
            vec![("old path".into(), Path::Old), ("new, all at 0".into(), Path::New(Opts::from_bits(0)))];
        for (k, name) in NAMES.iter().enumerate() {
            if 1 << k != DEDUP {
                paths.push((format!("{name} alone"), Path::New(Opts::from_bits(1 << k))));
                paths.push((format!("all except {name}"), Path::New(Opts::from_bits(all & !(1 << k)))));
            }
        }
        paths.push(("all (without LIGHTDEDUP)".into(), Path::New(Opts::from_bits(all))));
        paths.push(("all + LIGHTDEDUP".into(), Path::New(Opts::ALL)));
        let mut best = vec![f64::MAX; paths.len()];
        for _ in 0..7 {
            for (b, (_, p)) in best.iter_mut().zip(&paths) {
                *b = b.min(run(&t, &world, &tr, *p, masked, 1));
            }
        }
        let old = best[0];
        for ((name, _), b) in paths.iter().zip(&best) {
            eprintln!("{kind:9} {name:28} {b:9.1} µs ({:+.1} %)", (b / old - 1.0) * 100.0);
        }
        // Two calling threads at the same time (per-thread buffers) against one.
        for o in [Opts::from_bits(all), Opts::from_bits(all & !32)] {
            let one = run(&t, &world, &tr, Path::New(o), masked, 3);
            let two = std::thread::scope(|s| {
                let h: Vec<_> = (0..2).map(|_| s.spawn(|| run(&t, &world, &tr, Path::New(o), masked, 3))).collect();
                h.into_iter().map(|h| h.join().expect("thread")).fold(0.0, f64::max)
            });
            eprintln!("{kind:9} ZONEBUF={} 1 thread {one:9.1} µs, 2 threads {two:9.1} µs per relight", u8::from(o.zone_buf));
        }
    }
}

/// Source heights of the interior stacks of a sky zone, as the seeding computes them.
fn source_heights(world: &World, c: &Ctx, aux: &mut Aux, z: &Cells, g: Geo) -> Vec<i32> {
    let (sx, sz, ny) = (g.size[0], g.size[2], g.size[1]);
    let columns = SkyColumns::when_cached(world, c, aux, g);
    let mut heights = vec![i32::MAX; (sx * sz) as usize];
    for dz in 1..sz - 1 {
        for dx in 1..sx - 1 {
            heights[(dz * sx + dx) as usize] = column_source(world, c, aux, columns.as_ref(), z, g, 0, ny, dx, dz);
        }
    }
    heights
}

/// µs per group of the zone read, the block seeding, the sky sources and the sky seeding, replaying
/// `kind` from `world` with the switches `o` and the order of a group with `ZONEONCE` (one read,
/// block on its slice, then sky; each resolution propagated and written so the trace goes on).
fn seed_phases_us(t: &LightTables, base: &World, world: &World, kind: &str, o: Opts) -> [f64; 4] {
    let c = Ctx { height: H, t, o, count_queues: false };
    let mut world = world.clone();
    let (mut ns, mut count) = ([0u128; 4], 0usize);
    for step in trace(base, kind) {
        for undo in [false, true] {
            let ps: Vec<[i32; 3]> = step.iter().map(|&(p, _)| p).collect();
            for &(p, s) in &step {
                set(&mut world, p, if undo { 0 } else { s });
            }
            for g in groups(&ps) {
                let mut out = [Touched::new()];
                with_work(true, |w| {
                    w.aux.memo.clear();
                    let bg = block_zone(&c, &g);
                    let (sg, slo) = sky_zone(&world, &c, &mut w.aux, &g, bg);
                    let block_rows = SolvedRows { first: bg.lo[1] + 1 - slo, count: bg.size[1] };
                    let t0 = std::time::Instant::now();
                    read_zone(&world, &c, w, sg, Some(SolvedRows::all(sg)), Some(block_rows));
                    let t1 = std::time::Instant::now();
                    let Work { face, prop, emit, emitters, sky, blk, aux, lane, lane2, .. } = w;
                    let z = Cells { face, prop, emit, emitters };
                    let (r0b, nyb) = (block_rows.first, block_rows.count);
                    let block_lit = seed(&world, &c, &z, blk, lane2, None, None, sg, r0b, nyb, false);
                    let t2 = std::time::Instant::now();
                    let heights = source_heights(&world, &c, aux, &z, sg);
                    let t3 = std::time::Instant::now();
                    seed(&world, &c, &z, sky, lane, Some(aux), Some(&heights[..]), sg, 0, sg.size[1], true);
                    let t4 = std::time::Instant::now();
                    if block_lit {
                        propagate(&c, &z, blk, lane2, sg, r0b, nyb);
                        write(&mut world, &c, blk, lane2, sg, r0b, nyb, false, &mut out);
                    }
                    propagate(&c, &z, sky, lane, sg, 0, sg.size[1]);
                    write(&mut world, &c, sky, lane, sg, 0, sg.size[1], true, &mut out);
                    for (k, d) in [t1 - t0, t2 - t1, t3 - t2, t4 - t3].iter().enumerate() {
                        ns[k] += d.as_nanos();
                    }
                });
                count += 1;
            }
        }
    }
    ns.map(|v| v as f64 / 1000.0 / count as f64)
}

/// C2-M11 of series 3: zone read, block seeding, sky sources and sky seeding (µs per group, best of
/// 7 A B B A) before the mission (its two switches off), shipped, each switch alone on before and
/// shipped without each; then whole relights before / shipped (µs per relight, best of 7) outside a
/// pool, then in pools of 1, 2, 4… threads up to every core (slab and redstone: zones read and
/// sourced in parallel). `RUSTCRAFT_BENCH_KINDS=cave,plains` keeps some traces.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_seed_at_read -- --ignored --nocapture"]
fn bench_seed_at_read() {
    let t = tables();
    let base = bench_world(&t);
    let shipped = Opts::from_bits(shipped_bits());
    let before = Opts::from_bits(shipped_bits() & !SEED_AND_SKY_COLUMN_BITS);
    let mut paths = vec![("before".to_string(), before), ("shipped".to_string(), shipped)];
    for name in ["LIGHTSEEDATREAD", "LIGHTSKYCOLUMNCACHE"] {
        paths.push((format!("{name} alone"), before.with(name, true)));
        paths.push((format!("shipped without {name}"), shipped.with(name, false)));
    }
    // Opening after the read refines the prepared read: measured against shipped only.
    paths.push(("shipped without LIGHTOPENAFTERREAD".to_string(), shipped.with("LIGHTOPENAFTERREAD", false)));
    let only = rc_data::env_str!("RUSTCRAFT_BENCH_KINDS");
    let wanted = |kind: &str| only.is_none_or(|k| k.split(',').any(|x| x == kind));
    let kinds = ["plains", "torches", "cave", "redstone", "building", "water", "slab", "spread"];
    for kind in kinds.into_iter().filter(|k| wanted(k)) {
        let world = start_world(&t, &base, kind);
        let mut best = vec![[f64::MAX; 4]; paths.len()];
        for pass in 0..7 {
            let order: Vec<usize> = if pass % 2 == 0 { (0..paths.len()).collect() } else { (0..paths.len()).rev().collect() };
            for k in order {
                let us = seed_phases_us(&t, &base, &world, kind, paths[k].1);
                for (b, u) in best[k].iter_mut().zip(us) {
                    *b = b.min(u);
                }
            }
        }
        let b0 = best[0];
        for ((name, _), b) in paths.iter().zip(&best) {
            let rel = |k: usize| (b[k] / b0[k] - 1.0) * 100.0;
            eprintln!(
                "{kind:9} {name:34} read {:6.1} ({:+.0} %), block seeds {:6.1} ({:+.0} %), sky sources {:6.1} ({:+.0} %), sky seeds {:6.1} ({:+.0} %) µs",
                b[0], rel(0), b[1], rel(1), b[2], rel(2), b[3], rel(3)
            );
        }
    }
    for kind in kinds.into_iter().filter(|k| wanted(k)) {
        let world = start_world(&t, &base, kind);
        let tr = trace(&base, kind);
        let masked = kind == "redstone";
        let b = best_abba(&t, &world, &tr, &[before, shipped], masked, 7);
        eprintln!("{kind:9} relight outside pool: before {:9.1} µs, shipped {:9.1} µs ({:+.1} %)", b[0], b[1], (b[1] / b[0] - 1.0) * 100.0);
        for (name, o) in [("before", before), ("shipped", shipped)] {
            let stats_before = crate::stats::totals();
            let total = run(&t, &world, &tr, Path::New(o), masked, 1);
            let st = crate::stats::totals().since(&stats_before);
            let per = |ns: u64| ns as f64 / 1000.0 / (2 * tr.len()) as f64;
            eprintln!(
                "{kind:9}   {name:8} one more pass {total:7.1} µs per relight, of which sky solves {:6.1} µs ({} res.), block solves {:6.1} µs ({} res.)",
                per(st.sky_ns),
                st.sky_solves,
                per(st.block_ns),
                st.block_solves
            );
        }
        if kind != "slab" && kind != "redstone" {
            continue;
        }
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut threads: Vec<usize> = std::iter::successors(Some(1), |n| Some(n * 2)).take_while(|&n| n <= cores).collect();
        if threads.last() != Some(&cores) {
            threads.push(cores);
        }
        for n in threads {
            let b = super::pool(n).install(|| best_abba(&t, &world, &tr, &[before, shipped], masked, 5));
            eprintln!("{kind:9} pool {n:2} threads: before {:9.1} µs, shipped {:9.1} µs ({:+.1} %)", b[0], b[1], (b[1] / b[0] - 1.0) * 100.0);
        }
    }
}

/// C2-M12: every group by the zone (`LIGHTINC=0`, the working path of before), unchecked
/// incremental, checked (shipped, groups of at most `CHECKED_GROUP_MAX` changes), checked with no
/// limit and checked in order (`LIGHTINCOVERLAP`), the other switches shipped. Per trace: checked
/// resolutions kept and handed back, the check's share of the checked resolutions' time (what a cache of consistent sections could save at most), then µs per relight,
/// best of 7 A B B A, outside a pool (calling thread and light pool: the game's case), in a pool of
/// 1 thread (the whole CPU time) and of all the logical cores. `RUSTCRAFT_BENCH_KINDS` keeps some
/// traces.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_checked_incremental -- --ignored --nocapture"]
fn bench_checked_incremental() {
    use crate::fast::checked_inc::{CHECK_NS, CHECKED_CELLS, CHECKED_NS, CHECKS};
    let t = tables();
    let base = bench_world(&t);
    let zone = Opts::from_bits(shipped_bits());
    let shipped_max = crate::fast::inc::CHECKED_GROUP_MAX;
    let paths = [
        ("zone", zone),
        ("unchecked", Opts { inc: true, inc_verify: false, ..zone }),
        ("checked", Opts { inc: true, inc_max: shipped_max, ..zone }),
        ("no limit", Opts { inc: true, ..zone }),
        ("in order", Opts { inc: true, inc_max: shipped_max, inc_overlap: true, ..zone }),
    ];
    let opts: Vec<Opts> = paths.iter().map(|p| p.1).collect();
    let only = rc_data::env_str!("RUSTCRAFT_BENCH_KINDS");
    let wanted = |kind: &str| only.is_none_or(|k| k.split(',').any(|x| x == kind));
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let kinds = ["plains", "torches", "cave", "redstone", "building", "water", "slab", "spread"];
    for kind in kinds.into_iter().filter(|k| wanted(k)) {
        let world = start_world(&t, &base, kind);
        let tr = trace(&base, kind);
        let masked = kind == "redstone";
        for (name, o) in &paths[2..] {
            let (checks0, check0, checked0) = (CHECKS.with(|n| n.get()), CHECK_NS.with(|n| n.get()), CHECKED_NS.with(|n| n.get()));
            let cells0 = CHECKED_CELLS.with(|n| n.get());
            run(&t, &world, &tr, Path::New(*o), masked, 1);
            let (kept, fallbacks) = CHECKS.with(|n| n.get());
            let (check, checked) = (CHECK_NS.with(|n| n.get()) - check0, CHECKED_NS.with(|n| n.get()) - checked0);
            let (cells, with_faces) = CHECKED_CELLS.with(|n| n.get());
            let (cells, with_faces) = (cells - cells0.0, with_faces - cells0.1);
            eprintln!(
                "{kind:9} {name:9}: {} kept, {} handed back to the zone; the check is {:.0} % of the checked resolutions' \
                 time, {:.2} ns per cell walked ({} cells per relight, {:.2} % with faces)",
                kept - checks0.0,
                fallbacks - checks0.1,
                100.0 * check as f64 / checked.max(1) as f64,
                check as f64 / cells.max(1) as f64,
                cells / (2 * tr.len() as u64),
                100.0 * with_faces as f64 / cells.max(1) as f64
            );
        }
        for (place, threads) in [("outside a pool", None), ("pool of 1", Some(1)), ("pool of all", Some(cores))] {
            let best = match threads {
                None => best_abba(&t, &world, &tr, &opts, masked, 7),
                Some(n) => super::pool(n).install(|| best_abba(&t, &world, &tr, &opts, masked, 7)),
            };
            let line: Vec<String> =
                paths.iter().zip(&best).map(|((name, _), b)| format!("{name} {b:8.1} µs ({:+.0} %)", (b / best[0] - 1.0) * 100.0)).collect();
            eprintln!("{kind:9} {place:14}: {}", line.join(", "));
        }
    }
}

/// Squares of `side` × `side` blocks placed in the air at y = 150, 60 of them (each one group of
/// `side²` changes, the 16 × 16 one being the `slab` trace).
fn slab_trace(side: i32) -> Vec<Vec<([i32; 3], u32)>> {
    let mut rng = Rng(42);
    (0..60)
        .map(|_| {
            let (x, z) = (rng.below(32) - 16, rng.below(32) - 16);
            (0..side * side).map(|k| ([x + k % side, 150, z + k / side], 1)).collect()
        })
        .collect()
}

/// C2-M12, default `LIGHTINCMAX` of the checked path: the zone against the checked incremental with
/// no limit, on one group of growing size (squares in the air), best of 7 A B B A outside a pool and
/// in a pool of 1 thread. The size where the checked path stops gaining is the default limit.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_checked_group_size -- --ignored --nocapture"]
fn bench_checked_group_size() {
    let t = tables();
    let world = bench_world(&t);
    let zone = Opts::from_bits(shipped_bits());
    let opts = [zone, Opts { inc: true, ..zone }];
    for side in [2, 4, 6, 8, 10, 12, 14, 16] {
        let tr = slab_trace(side);
        for (place, threads) in [("outside a pool", None), ("pool of 1", Some(1))] {
            let best = match threads {
                None => best_abba(&t, &world, &tr, &opts, false, 7),
                Some(n) => super::pool(n).install(|| best_abba(&t, &world, &tr, &opts, false, 7)),
            };
            eprintln!(
                "{:3} changes, {place:14}: zone {:8.1} µs, checked {:8.1} µs ({:+.0} %)",
                side * side,
                best[0],
                best[1],
                (best[1] / best[0] - 1.0) * 100.0
            );
        }
    }
}
