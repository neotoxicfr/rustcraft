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
/// propagation, write), shipped sequential path, µs per group: where multi-core can win.
#[test]
#[ignore = "bench: cargo test --release -p rc-light bench_phases -- --ignored --nocapture"]
fn bench_phases() {
    let t = tables();
    let base = bench_world(&t);
    let o = Opts::from_bits(!super::DEFAULT_OFF & 0x7ff);
    let c = Ctx { height: H, t: &t, o };
    for kind in ["plains", "torches", "building", "slab", "spread"] {
        let mut world = start_world(&t, &base, kind);
        let mut ns = [0u128; 5];
        let mut cells = 0usize;
        let mut count = 0usize;
        for step in trace(&base, kind) {
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
                        let Work { face, prop, emit, sky, aux, lane, .. } = w;
                        let z = Cells { face, prop, emit };
                        seed(&world, &c, &z, sky, lane, Some(aux), None, sg, 0, sg.size[1], true);
                        let t3 = std::time::Instant::now();
                        propagate(&c, &z, sky, lane, sg, 0, sg.size[1]);
                        let t4 = std::time::Instant::now();
                        write(&mut world, sky, lane, sg, 0, sg.size[1], true, &mut out);
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
    let o = |bits: u16, zero: bool| {
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
