//! Iteration order of `JSet` against a real Java hash set (`jset_java/JSetTrace.java`,
//! JDK only): same operation sequences, fingerprints stored in `jset_java/expected.txt`.
//! Regenerate: `java crates/rc-worldgen/tests/jset_java/JSetTrace.java <expected.txt> 1024`.
//! `cargo test --release -p rc-worldgen --test jset_java`

use rc_worldgen::jset::{JSet, treeified_count};

struct Gen(i64);

impl Gen {
    fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 as u64) >> 33) as i32
    }
    fn bounded(&mut self, n: i32) -> i32 {
        self.next() % n
    }
}

struct Fnv(u64);

impl Fnv {
    fn mix(&mut self, v: i32) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

/// Replays sequence `k` (same draw as the Java program); returns its fingerprint.
fn run(k: i64, keep: bool) -> (u64, usize) {
    let mut g = Gen(k.wrapping_mul(0x9E3779B97F4A7C15u64 as i64).wrapping_add(12345));
    let mut h = Fnv(0xcbf29ce484222325);
    let (ox, oy, oz) = (g.bounded(2000000) - 1000000, g.bounded(384) - 64, g.bounded(2000000) - 1000000);
    let (sx, mut sy, sz) = (4 + g.bounded(21), 4 + g.bounded(27), 4 + g.bounded(21));
    // Shapes: box, thin slice (same y), diagonal (1, 1, 2): on this diagonal the hash
    // advances by 1024, all its positions fall in the same bucket up to 1024 buckets.
    let shape = g.bounded(3);
    if shape == 1 {
        sy = 1 + g.bounded(3);
    }
    let ops = 200 + g.bounded(8000);
    let pop = [10, 35, 50][g.bounded(3) as usize];
    let mut set = JSet::with_keep(keep);
    let mut with_trees = 0;
    for o in 0..=ops {
        with_trees += usize::from(set.tree_bins() > 0);
        let r = if o == ops { 0 } else { g.bounded(100) };
        if r < 2 {
            h.mix(4);
            h.mix(set.len() as i32);
            for p in set.iter() {
                h.mix(p[0]);
                h.mix(p[1]);
                h.mix(p[2]);
            }
        } else if r < 2 + pop {
            match set.pop_first() {
                Some(p) => {
                    h.mix(2);
                    h.mix(p[0]);
                    h.mix(p[1]);
                    h.mix(p[2]);
                }
                None => h.mix(3),
            }
        } else if r == 99 && g.bounded(20) == 0 {
            set.clear();
            h.mix(5);
        } else {
            let p = if shape == 2 {
                let d = g.bounded(24);
                [ox + g.bounded(3) + d, oy + g.bounded(3) + d, oz + g.bounded(3) + 2 * d]
            } else {
                [ox + g.bounded(sx), oy + g.bounded(sy), oz + g.bounded(sz)]
            };
            let added = set.insert(p);
            h.mix(1);
            h.mix(i32::from(added));
        }
    }
    (h.0, with_trees)
}

#[test]
fn jset_matches_java_hash_set() {
    let expected: Vec<u64> = include_str!("jset_java/expected.txt")
        .lines()
        .map(|l| u64::from_str_radix(l.split(' ').nth(1).expect("fingerprint"), 16).expect("hexadecimal"))
        .collect();
    assert_eq!(expected.len(), 1024, "incomplete Java trace");
    for keep in [true, false] {
        let runs: Vec<(u64, usize)> = (0..expected.len()).map(|k| run(k as i64, keep)).collect();
        let bad: Vec<usize> = (0..expected.len()).filter(|&k| runs[k].0 != expected[k]).collect();
        assert!(bad.is_empty(), "keep={keep}: {} sequences differ from Java, first {:?}", bad.len(), bad.first());
        // The trace must exercise the tree buckets, otherwise it proves nothing.
        let ops: usize = runs.iter().map(|r| r.1).sum();
        let seqs = runs.iter().filter(|r| r.1 > 0).count();
        eprintln!("keep={keep}: {seqs} sequences and {ops} operations with a tree bucket (total {})", treeified_count());
        assert!(seqs >= 40, "too few sequences with a tree bucket ({seqs})");
    }
}

/// Micro-bench of the two paths on the equivalence sequences (1 thread; best of 5).
/// `cargo test --release -p rc-worldgen --test jset_java jset_keep_bench -- --ignored --nocapture`
#[test]
#[ignore = "benchmark"]
fn jset_keep_bench() {
    for keep in [false, true, false, true] {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let mut acc = 0u64;
            for k in 0..2000i64 {
                acc ^= run(k + 1_000_000, keep).0;
            }
            std::hint::black_box(acc);
            best = best.min(t.elapsed().as_secs_f64());
        }
        eprintln!("keep={keep}: {:.1} ms for 2000 sequences", best * 1e3);
    }
}

/// Old and new path (`RUSTCRAFT_OPT_JSET_KEEP`) identical over 10⁴ sequences.
#[test]
fn jset_keep_matches_old_path() {
    for k in 0..10_000i64 {
        assert_eq!(run(k + 1_000_000, true).0, run(k + 1_000_000, false).0, "suite {k}");
    }
}
