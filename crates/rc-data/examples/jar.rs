//! Micro-bench of the jar index (`rc_data::jar`): old readers (unbuffered file, each
//! entry opened) against the index according to I/O, name filter and number of threads
//! (1 then powers of 2 up to the number of logical cores).
//!
//! `cargo run --release -p rc-data --example jar [-- <jar>] [repetitions]`

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rc_data::jar::{Io, Opts, build};

/// Old reader: unbuffered file, each entry opened then sorted by name.
fn old_reader(path: &Path, keep: fn(&str) -> bool) -> usize {
    let f = std::fs::File::open(path).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    let mut n = 0;
    for i in 0..z.len() {
        let Ok(mut e) = z.by_index(i) else { continue };
        let Ok(name) = e.name().map(|n| n.into_owned()) else { continue };
        if !keep(&name) {
            continue;
        }
        let mut v = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut v).unwrap();
        n += v.len();
    }
    n
}

fn registries(n: &str) -> bool {
    n.starts_with("data/") && n.ends_with(".json")
}
fn worldgen(n: &str) -> bool {
    n.starts_with("data/") && (n.contains("/worldgen/") || n.contains("/tags/block/"))
}
fn decor(n: &str) -> bool {
    n.starts_with("data/") && (n.contains("/worldgen/") || n.contains("/tags/") || n.contains("/structure/"))
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

fn time(reps: usize, mut f: impl FnMut()) -> (f64, f64) {
    f();
    stats(
        (0..reps)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect(),
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    let jar = args.next().map(PathBuf::from).unwrap_or_else(|| rc_data::locate("26.3").expect("jar 26.3").client_jar);
    let reps: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(7);
    println!("jar {}; {reps} repetitions; min / median in ms (warm system cache)", jar.display());
    for (what, keep) in [("registries", registries as fn(&str) -> bool), ("generation", worldgen), ("decor", decor)] {
        let (a, m) = time(reps, || {
            std::hint::black_box(old_reader(&jar, keep));
        });
        println!("old reader     {what:<11} {a:8.1} {m:8.1}");
    }
    let (a, m) = time(reps, || {
        std::hint::black_box(std::fs::read(&jar).unwrap());
    });
    println!("fs::read alone            {a:8.1} {m:8.1}");
    let bytes: std::sync::Arc<[u8]> = std::fs::read(&jar).unwrap().into();
    let (a, m) = time(reps, || {
        std::hint::black_box(zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap());
    });
    println!("central directory only    {a:8.1} {m:8.1}");
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut threads = vec![1];
    while threads.last().copied().unwrap_or(1) * 2 <= cores {
        threads.push(threads.last().copied().unwrap_or(1) * 2);
    }
    if threads.last() != Some(&cores) {
        threads.push(cores);
    }
    for io in [Io::File, Io::Read, Io::Mmap] {
        for filter in [false, true] {
            let ts: &[usize] = if io == Io::File || !filter { &[1] } else { &threads };
            for &n in ts {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
                let o = Opts { io, filter, threads: n };
                let (a, m) = pool.install(|| {
                    time(reps, || {
                        std::hint::black_box(build(&jar, o).unwrap());
                    })
                });
                println!("index {io:?} filter={filter} threads={n:<3} {a:8.1} {m:8.1}");
            }
        }
    }
}
