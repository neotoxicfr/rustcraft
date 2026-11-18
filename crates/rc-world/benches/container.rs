use std::hint::black_box;
use std::time::Instant;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use rc_world::{Container, GetMode, Section, block_index};

/// Scattered values (`distinct` states): worst case of the runs.
fn scattered(distinct: u32) -> Vec<u32> {
    (0..4096u32).map(|i| (i * 7919) % distinct).collect()
}

/// Terrain section: layers (stone, dirt, grass, air) with a few ores.
fn terrain() -> Vec<u32> {
    (0..4096u32)
        .map(|i| {
            let y = i >> 8;
            match y {
                0..=8 if i % 97 == 0 => 20 + i % 5,
                0..=8 => 1,
                9..=10 => 10,
                11 => 9,
                _ => 0,
            }
        })
        .collect()
}

const MODES: [(&str, GetMode); 3] = [("ref", GetMode::Reference), ("magic", GetMode::Magic), ("mono", GetMode::Mono)];

/// Read of a whole section: one-by-one access (each path) against block decompression.
fn read_section(c: &mut Criterion) {
    for distinct in [12u32, 50, 5000] {
        let cont = Container::pack_reference(&scattered(distinct), 4, 8, 16);
        let mut out = vec![0u32; 4096];
        c.bench_function(&format!("get_each_{distinct}"), |b| {
            b.iter(|| {
                for (i, o) in out.iter_mut().enumerate() {
                    *o = cont.get(i);
                }
                black_box(&out);
            })
        });
        for (name, mode) in MODES {
            c.bench_function(&format!("get_each_{name}_{distinct}"), |b| {
                b.iter(|| {
                    for (i, o) in out.iter_mut().enumerate() {
                        *o = cont.get_with(i, mode);
                    }
                    black_box(&out);
                })
            });
        }
        c.bench_function(&format!("unpack_ref_{distinct}"), |b| {
            b.iter(|| {
                cont.unpack_reference(&mut out);
                black_box(&out);
            })
        });
        c.bench_function(&format!("unpack_mono_{distinct}"), |b| {
            b.iter(|| {
                cont.unpack_mono(&mut out);
                black_box(&out);
            })
        });
        c.bench_function(&format!("count_unpack_{distinct}"), |b| {
            b.iter(|| {
                cont.unpack_into(&mut out);
                black_box(out.iter().filter(|&&v| v != 0).count())
            })
        });
        c.bench_function(&format!("count_where_{distinct}"), |b| {
            b.iter(|| black_box(cont.count_where(4096, |v| v != 0)))
        });
        // No palette entry answers (section without fluid): exit before the histogram.
        c.bench_function(&format!("count_where_none_{distinct}"), |b| {
            b.iter(|| black_box(cont.count_where(4096, |v| black_box(v) > 1_000_000)))
        });
    }
}

/// Dependent reads (latency): the next index depends on the value read, by width.
fn get_latency(c: &mut Criterion) {
    for bits in [4u8, 5, 7, 15] {
        let distinct = if bits == 15 { 5000 } else { 1u32 << (bits - 1) | 1 };
        let cont = Container::pack_reference(&scattered(distinct), 4, 8, 15);
        assert!(matches!(&cont, Container::Packed { bits: b, .. } if *b == bits), "expected width {bits}");
        for (name, mode) in MODES {
            c.bench_function(&format!("get_chain_{name}_{bits}b"), |b| {
                b.iter(|| {
                    let mut i = 17usize;
                    let mut acc = 0u32;
                    for _ in 0..4096 {
                        let v = cont.get_with(i, mode);
                        acc = acc.wrapping_add(v);
                        i = (i * 1_103_515_245 + v as usize + 12_345) & 4095;
                    }
                    black_box(acc)
                })
            });
        }
    }
}

/// Vertical stacks: 256 stacks of 16 per `get` against the stack iterator.
fn columns(c: &mut Criterion) {
    for distinct in [12u32, 50] {
        let cont = Container::pack_reference(&scattered(distinct), 4, 8, 16);
        c.bench_function(&format!("column_get_{distinct}"), |b| {
            b.iter(|| {
                let mut acc = 0u32;
                for x in 0..16 {
                    for z in 0..16 {
                        for y in 0..16 {
                            acc = acc.wrapping_add(cont.get(block_index(x, y, z)));
                        }
                    }
                }
                black_box(acc)
            })
        });
        c.bench_function(&format!("column_iter_{distinct}"), |b| {
            b.iter(|| {
                let mut acc = 0u32;
                for x in 0..16 {
                    for z in 0..16 {
                        for v in cont.column_iter(x, z, 0..16) {
                            acc = acc.wrapping_add(v);
                        }
                    }
                }
                black_box(acc)
            })
        });
    }
}

const PACKS: [(&str, bool, bool); 4] =
    [("ref", false, false), ("table", true, false), ("run", false, true), ("table_run", true, true)];

/// Palettisation of a section: old code against reverse table, run shortcut, both.
fn pack(c: &mut Criterion) {
    let mut sets: Vec<(String, Vec<u32>)> =
        [12u32, 50, 256, 5000].into_iter().map(|d| (d.to_string(), scattered(d))).collect();
    sets.push(("terrain".into(), terrain()));
    for (label, values) in &sets {
        for (name, table, run) in PACKS {
            c.bench_function(&format!("pack_{name}_{label}"), |b| {
                b.iter(|| black_box(Container::pack_with(black_box(values), 4, 8, 15, table, run)))
            });
        }
    }
}

/// Palettisation throughput on all cores (1 thread then powers of 2): 2,048 sections per
/// measurement, split by thread; printed on the error output (outside criterion).
fn pack_threads(_c: &mut Criterion) {
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    let values = scattered(50);
    let terrain = terrain();
    let mut threads = vec![1usize];
    while threads.last().unwrap() * 2 <= n {
        threads.push(threads.last().unwrap() * 2);
    }
    if *threads.last().unwrap() != n {
        threads.push(n);
    }
    for (label, v) in [("50", &values), ("terrain", &terrain)] {
        for (name, table, run) in PACKS {
            for &t in &threads {
                let per = 2048 / t;
                let best = (0..5)
                    .map(|_| {
                        let t0 = Instant::now();
                        std::thread::scope(|s| {
                            for _ in 0..t {
                                s.spawn(|| {
                                    for _ in 0..per {
                                        black_box(Container::pack_with(black_box(v), 4, 8, 15, table, run));
                                    }
                                });
                            }
                        });
                        t0.elapsed().as_secs_f64()
                    })
                    .fold(f64::MAX, f64::min);
                eprintln!("pack_threads {label} {name} {t} threads: {:.1} µs/section (throughput)", best * 1e6 / 2048.0);
            }
        }
    }
}

/// Update of one block (network packet): full repacking against in-place write, and
/// growth of a full palette (repack against widening).
fn set_block(c: &mut Criterion) {
    for distinct in [12u32, 50, 5000] {
        let base =
            Section { non_empty: 0, blocks: Container::pack(&scattered(distinct), 4, 8, 16), biomes: Container::Single(0) };
        let changes = [(1234u16, 3u32)];
        c.bench_function(&format!("set_repack_{distinct}"), |b| {
            let mut s = base.clone();
            b.iter(|| s.set_blocks_repack(black_box(&changes), 16, |v| v == 0))
        });
        c.bench_function(&format!("set_in_place_{distinct}"), |b| {
            let mut s = base.clone();
            b.iter(|| s.set_blocks(black_box(&changes), 16, |v| v == 0))
        });
    }
    // 4-bit palette with 15 entries: the 16th enters in place (growth), the 17th overflows.
    let fifteen = Container::pack_reference(&scattered(15), 4, 8, 15);
    let sixteen = Container::pack_reference(&scattered(16), 4, 8, 15);
    for (name, cap) in [("old", false), ("cap", true)] {
        c.bench_function(&format!("set_grow_{name}"), |b| {
            b.iter_batched(
                || fifteen.clone(),
                |mut k| {
                    k.set_with(7, 999, 4, 8, 15, cap, false);
                    k
                },
                BatchSize::SmallInput,
            )
        });
    }
    for (name, cap, widen) in [("old", false, false), ("cap", true, false), ("widen", true, true)] {
        c.bench_function(&format!("set_overflow_{name}"), |b| {
            b.iter_batched(
                || sixteen.clone(),
                |mut k| {
                    k.set_with(7, 999, 4, 8, 15, cap, widen);
                    k
                },
                BatchSize::SmallInput,
            )
        });
    }
}

criterion_group!(benches, read_section, get_latency, columns, pack, pack_threads, set_block);
criterion_main!(benches);
